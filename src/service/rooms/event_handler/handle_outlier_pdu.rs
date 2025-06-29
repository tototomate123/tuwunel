use std::collections::{HashMap, hash_map};

use ruma::{
	CanonicalJsonObject, CanonicalJsonValue, EventId, RoomId, RoomVersionId, ServerName,
	events::{StateEventType, TimelineEventType},
};
use tuwunel_core::{
	Err, Result, debug, debug_info, err, implement,
	matrix::{Event, PduEvent, event::TypeExt, room_version},
	state_res, trace, warn,
};

use super::check_room_id;

#[implement(super::Service)]
pub(super) async fn handle_outlier_pdu(
	&self,
	origin: &ServerName,
	room_id: &RoomId,
	event_id: &EventId,
	mut pdu_json: CanonicalJsonObject,
	room_version: &RoomVersionId,
	auth_events_known: bool,
) -> Result<(PduEvent, CanonicalJsonObject)> {
	// 1. Remove unsigned field
	pdu_json.remove("unsigned");

	// TODO: For RoomVersion6 we must check that Raw<..> is canonical do we
	// anywhere?: https://matrix.org/docs/spec/rooms/v6#canonical-json
	// 2. Check signatures, otherwise drop
	// 3. check content hash, redact if doesn't match
	let mut pdu_json = match self
		.services
		.server_keys
		.verify_event(&pdu_json, Some(room_version))
		.await
	{
		| Ok(ruma::signatures::Verified::All) => pdu_json,
		| Ok(ruma::signatures::Verified::Signatures) => {
			// Redact
			debug_info!("Calculated hash does not match (redaction): {event_id}");
			let Some(rules) = room_version.rules() else {
				return Err!(Request(UnsupportedRoomVersion(
					"Cannot redact event for unknown room version {room_version:?}."
				)));
			};

			let Ok(obj) = ruma::canonical_json::redact(pdu_json, &rules.redaction, None) else {
				return Err!(Request(InvalidParam("Redaction failed")));
			};

			// Skip the PDU if it is redacted and we already have it as an outlier event
			if self.services.timeline.pdu_exists(event_id).await {
				return Err!(Request(InvalidParam(
					"Event was redacted and we already knew about it"
				)));
			}

			obj
		},
		| Err(e) => {
			return Err!(Request(InvalidParam(debug_error!(
				"Signature verification failed for {event_id}: {e}"
			))));
		},
	};

	// Now that we have checked the signature and hashes we can add the eventID and
	// convert to our PduEvent type
	pdu_json.insert("event_id".to_owned(), CanonicalJsonValue::String(event_id.to_string()));

	let event = serde_json::from_value::<PduEvent>(serde_json::to_value(&pdu_json)?)
		.map_err(|e| err!(Request(BadJson(debug_warn!("Event is not a valid PDU: {e}")))))?;

	check_room_id(room_id, &event)?;

	if !auth_events_known {
		// 4. fetch any missing auth events doing all checks listed here starting at 1.
		//    These are not timeline events
		// 5. Reject "due to auth events" if can't get all the auth events or some of
		//    the auth events are also rejected "due to auth events"
		// NOTE: Step 5 is not applied anymore because it failed too often
		debug!("Fetching auth events");
		Box::pin(self.fetch_auth(origin, room_id, event.auth_events(), room_version)).await;
	}

	// 6. Reject "due to auth events" if the event doesn't pass auth based on the
	//    auth events
	debug!("Checking based on auth events");

	let room_rules = room_version::rules(room_version)?;
	let is_create = *event.kind() == TimelineEventType::RoomCreate;
	let is_hydra = room_rules
		.authorization
		.room_create_event_id_as_room_id;

	let hydra_create_id = (is_hydra && !is_create).then_some(event.room_id().as_event_id()?);
	let auth_event_ids = event
		.auth_events()
		.map(ToOwned::to_owned)
		.chain(hydra_create_id.into_iter());

	// Build map of auth events
	let mut auth_events = HashMap::with_capacity(event.auth_events().count().saturating_add(1));
	for id in auth_event_ids {
		let Ok(auth_event) = self.services.timeline.get_pdu(&id).await else {
			warn!("Could not find auth event {id}");
			continue;
		};

		check_room_id(room_id, &auth_event)?;
		match auth_events.entry((
			auth_event.kind.to_string().into(),
			auth_event
				.state_key
				.clone()
				.expect("all auth events have state keys"),
		)) {
			| hash_map::Entry::Vacant(v) => {
				v.insert(auth_event);
			},
			| hash_map::Entry::Occupied(_) => {
				return Err!(Request(InvalidParam(
					"Auth event's type and state_key combination exists multiple times.",
				)));
			},
		}
	}

	// The original create event must be in the auth events
	if !matches!(
		auth_events.get(&(StateEventType::RoomCreate, String::new().into())),
		Some(_) | None
	) {
		return Err!(Request(InvalidParam("Incoming event refers to wrong create event.")));
	}

	state_res::auth_check(
		&room_rules,
		&event,
		&async |event_id| self.event_fetch(&event_id).await,
		&async |event_type, state_key| {
			auth_events
				.get(&event_type.with_state_key(state_key.as_str()))
				.map(ToOwned::to_owned)
				.ok_or_else(|| err!(Request(NotFound("state not found"))))
		},
	)
	.await
	.map_err(|e| err!(Request(Forbidden("Auth check failed: {e:?}"))))?;

	trace!("Validation successful.");

	// 7. Persist the event as an outlier.
	self.services
		.timeline
		.add_pdu_outlier(event.event_id(), &pdu_json);

	trace!("Added pdu as outlier.");

	Ok((event, pdu_json))
}
