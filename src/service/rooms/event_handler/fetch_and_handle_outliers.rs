use std::{
	collections::{HashSet, VecDeque},
	ops::Range,
	time::Duration,
};

use ruma::{
	CanonicalJsonObject, CanonicalJsonValue, EventId, OwnedEventId, RoomId, ServerName,
	api::federation::event::get_event,
};
use tuwunel_core::{
	debug, debug_error, debug_warn, implement,
	matrix::{
		PduEvent,
		event::{Event, gen_event_id_canonical_json},
	},
	trace, warn,
};

use super::get_room_version_id;

/// Find the event and auth it. Once the event is validated (steps 1 - 8)
/// it is appended to the outliers Tree.
///
/// Returns pdu and if we fetched it over federation the raw json.
///
/// a. Look in the main timeline (pduid_pdu tree)
/// b. Look at outlier pdu tree
/// c. Ask origin server over federation
/// d. TODO: Ask other servers over federation?
#[implement(super::Service)]
pub(super) async fn fetch_and_handle_outliers<'a, Pdu, Events>(
	&self,
	origin: &'a ServerName,
	events: Events,
	create_event: &'a Pdu,
	room_id: &'a RoomId,
) -> Vec<(PduEvent, Option<CanonicalJsonObject>)>
where
	Pdu: Event,
	Events: Iterator<Item = &'a EventId> + Clone + Send,
{
	let mut events_with_auth_events = Vec::with_capacity(events.clone().count());

	for event_id in events {
		let outlier = self
			.fetch_auth(room_id, event_id, origin, create_event)
			.await;

		events_with_auth_events.push(outlier);
	}

	let mut pdus = Vec::with_capacity(events_with_auth_events.len());
	for (id, local_pdu, events_in_reverse_order) in events_with_auth_events {
		// a. Look in the main timeline (pduid_pdu tree)
		// b. Look at outlier pdu tree
		// (get_pdu_json checks both)
		if let Some(local_pdu) = local_pdu {
			trace!("Found {id} in db");
			pdus.push((local_pdu.clone(), None));
		}

		for (next_id, value) in events_in_reverse_order.into_iter().rev() {
			if self.is_backed_off(&next_id, Range {
				start: Duration::from_secs(5 * 60),
				end: Duration::from_secs(60 * 60 * 24),
			}) {
				debug_warn!("Backing off from {next_id}");
				continue;
			}

			match Box::pin(self.handle_outlier_pdu(
				origin,
				create_event,
				&next_id,
				room_id,
				value.clone(),
				true,
			))
			.await
			{
				| Ok((pdu, json)) =>
					if next_id == *id {
						pdus.push((pdu, Some(json)));
					},
				| Err(e) => {
					warn!("Authentication of event {next_id} failed: {e:?}");
					self.back_off(&next_id);
				},
			}
		}
	}

	pdus
}

#[implement(super::Service)]
async fn fetch_auth<'a, Pdu>(
	&self,
	_room_id: &'a RoomId,
	event_id: &'a EventId,
	origin: &'a ServerName,
	create_event: &'a Pdu,
) -> (OwnedEventId, Option<PduEvent>, Vec<(OwnedEventId, CanonicalJsonObject)>)
where
	Pdu: Event,
{
	// a. Look in the main timeline (pduid_pdu tree)
	// b. Look at outlier pdu tree
	// (get_pdu_json checks both)
	if let Ok(local_pdu) = self.services.timeline.get_pdu(event_id).await {
		return (event_id.to_owned(), Some(local_pdu), vec![]);
	}

	// c. Ask origin server over federation
	// We also handle its auth chain here so we don't get a stack overflow in
	// handle_outlier_pdu.
	let mut todo_auth_events: VecDeque<_> = [event_id.to_owned()].into();
	let mut events_in_reverse_order = Vec::with_capacity(todo_auth_events.len());

	let mut events_all = HashSet::with_capacity(todo_auth_events.len());
	while let Some(next_id) = todo_auth_events.pop_front() {
		if self.is_backed_off(&next_id, Range {
			start: Duration::from_secs(2 * 60),
			end: Duration::from_secs(60 * 60 * 8),
		}) {
			debug_warn!("Backing off from {next_id}");
			continue;
		}

		if events_all.contains(&next_id) {
			continue;
		}

		if self.services.timeline.pdu_exists(&next_id).await {
			trace!("Found {next_id} in db");
			continue;
		}

		debug!("Fetching {next_id} over federation.");
		match self
			.services
			.sending
			.send_federation_request(origin, get_event::v1::Request {
				event_id: (*next_id).to_owned(),
				include_unredacted_content: None,
			})
			.await
		{
			| Ok(res) => {
				debug!("Got {next_id} over federation");
				let Ok(room_version_id) = get_room_version_id(create_event) else {
					self.back_off(&next_id);
					continue;
				};

				let Ok((calculated_event_id, value)) =
					gen_event_id_canonical_json(&res.pdu, &room_version_id)
				else {
					self.back_off(&next_id);
					continue;
				};

				if calculated_event_id != *next_id {
					warn!(
						"Server didn't return event id we requested: requested: {next_id}, we \
						 got {calculated_event_id}. Event: {:?}",
						&res.pdu
					);
				}

				if let Some(auth_events) = value
					.get("auth_events")
					.and_then(CanonicalJsonValue::as_array)
				{
					for auth_event in auth_events {
						match serde_json::from_value::<OwnedEventId>(auth_event.clone().into()) {
							| Ok(auth_event) => {
								todo_auth_events.push_back(auth_event);
							},
							| _ => {
								warn!("Auth event id is not valid");
							},
						}
					}
				} else {
					warn!("Auth event list invalid");
				}

				events_in_reverse_order.push((next_id.clone(), value));
				events_all.insert(next_id);
			},
			| Err(e) => {
				debug_error!("Failed to fetch event {next_id}: {e}");
				self.back_off(&next_id);
			},
		}
	}

	(event_id.to_owned(), None, events_in_reverse_order)
}
