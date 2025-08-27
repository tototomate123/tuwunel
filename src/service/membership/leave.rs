use std::collections::HashSet;

use futures::{FutureExt, StreamExt, TryFutureExt, pin_mut};
use ruma::{
	CanonicalJsonObject, CanonicalJsonValue, OwnedServerName, RoomId, RoomVersionId, UserId,
	api::federation,
	events::{
		StateEventType,
		room::member::{MembershipState, RoomMemberEventContent},
	},
};
use tuwunel_core::{
	Err, Result, debug_info, debug_warn, err, implement,
	matrix::event::gen_event_id,
	pdu::PduBuilder,
	utils::{self, FutureBoolExt, future::ReadyEqExt},
	warn,
};

use super::Service;
use crate::rooms::timeline::RoomMutexGuard;

#[implement(Service)]
#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(%room_id, %user_id)
)]
pub async fn leave(
	&self,
	user_id: &UserId,
	room_id: &RoomId,
	reason: Option<String>,
	state_lock: &RoomMutexGuard,
) -> Result {
	let default_member_content = RoomMemberEventContent {
		membership: MembershipState::Leave,
		reason: reason.clone(),
		join_authorized_via_users_server: None,
		is_direct: None,
		avatar_url: None,
		displayname: None,
		third_party_invite: None,
		blurhash: None,
	};

	let is_banned = self.services.metadata.is_banned(room_id);
	let is_disabled = self.services.metadata.is_disabled(room_id);

	pin_mut!(is_banned, is_disabled);
	if is_banned.or(is_disabled).await {
		// the room is banned/disabled, the room must be rejected locally since we
		// cant/dont want to federate with this server
		self.services
			.state_cache
			.update_membership(
				room_id,
				user_id,
				default_member_content,
				user_id,
				None,
				None,
				true,
			)
			.await?;

		return Ok(());
	}

	let dont_have_room = self
		.services
		.state_cache
		.server_in_room(self.services.globals.server_name(), room_id)
		.eq(&false);

	let not_knocked = self
		.services
		.state_cache
		.is_knocked(user_id, room_id)
		.eq(&false);

	// Ask a remote server if we don't have this room and are not knocking on it
	if dont_have_room.and(not_knocked).await {
		if let Err(e) = self.remote_leave(user_id, room_id).boxed().await {
			warn!(%user_id, "Failed to leave room {room_id} remotely: {e}");
			// Don't tell the client about this error
		}

		let last_state = self
			.services
			.state_cache
			.invite_state(user_id, room_id)
			.or_else(|_| {
				self.services
					.state_cache
					.knock_state(user_id, room_id)
			})
			.or_else(|_| {
				self.services
					.state_cache
					.left_state(user_id, room_id)
			})
			.await
			.ok();

		// We always drop the invite, we can't rely on other servers
		self.services
			.state_cache
			.update_membership(
				room_id,
				user_id,
				default_member_content,
				user_id,
				last_state,
				None,
				true,
			)
			.await?;
	} else {
		let Ok(event) = self
			.services
			.state_accessor
			.room_state_get_content::<RoomMemberEventContent>(
				room_id,
				&StateEventType::RoomMember,
				user_id.as_str(),
			)
			.await
		else {
			debug_warn!(
				"Trying to leave a room you are not a member of, marking room as left locally."
			);

			return self
				.services
				.state_cache
				.update_membership(
					room_id,
					user_id,
					default_member_content,
					user_id,
					None,
					None,
					true,
				)
				.await;
		};

		self.services
			.timeline
			.build_and_append_pdu(
				PduBuilder::state(user_id.to_string(), &RoomMemberEventContent {
					membership: MembershipState::Leave,
					reason,
					join_authorized_via_users_server: None,
					is_direct: None,
					..event
				}),
				user_id,
				room_id,
				state_lock,
			)
			.await?;
	}

	Ok(())
}

#[implement(Service)]
#[tracing::instrument(name = "remote", level = "debug", skip_all)]
pub async fn remote_leave(&self, user_id: &UserId, room_id: &RoomId) -> Result {
	let mut make_leave_response_and_server =
		Err!(BadServerResponse("No remote server available to assist in leaving {room_id}."));

	let mut servers: HashSet<OwnedServerName> = self
		.services
		.state_cache
		.servers_invite_via(room_id)
		.map(ToOwned::to_owned)
		.collect()
		.await;

	match self
		.services
		.state_cache
		.invite_state(user_id, room_id)
		.await
	{
		| Ok(invite_state) => {
			servers.extend(
				invite_state
					.iter()
					.filter_map(|event| event.get_field("sender").ok().flatten())
					.filter_map(|sender: &str| UserId::parse(sender).ok())
					.map(|user| user.server_name().to_owned()),
			);
		},
		| _ => {
			match self
				.services
				.state_cache
				.knock_state(user_id, room_id)
				.await
			{
				| Ok(knock_state) => {
					servers.extend(
						knock_state
							.iter()
							.filter_map(|event| event.get_field("sender").ok().flatten())
							.filter_map(|sender: &str| UserId::parse(sender).ok())
							.filter_map(|sender| {
								if !self.services.globals.user_is_local(sender) {
									Some(sender.server_name().to_owned())
								} else {
									None
								}
							}),
					);
				},
				| _ => {},
			}
		},
	}

	if let Some(room_id_server_name) = room_id.server_name() {
		servers.insert(room_id_server_name.to_owned());
	}

	debug_info!("servers in remote_leave_room: {servers:?}");

	for remote_server in servers {
		let make_leave_response = self
			.services
			.sending
			.send_federation_request(
				&remote_server,
				federation::membership::prepare_leave_event::v1::Request {
					room_id: room_id.to_owned(),
					user_id: user_id.to_owned(),
				},
			)
			.await;

		make_leave_response_and_server = make_leave_response.map(|r| (r, remote_server));

		if make_leave_response_and_server.is_ok() {
			break;
		}
	}

	let (make_leave_response, remote_server) = make_leave_response_and_server?;

	let Some(room_version_id) = make_leave_response.room_version else {
		return Err!(BadServerResponse(warn!(
			"No room version was returned by {remote_server} for {room_id}, room version is \
			 likely not supported by tuwunel"
		)));
	};

	if !self
		.services
		.server
		.supported_room_version(&room_version_id)
	{
		return Err!(BadServerResponse(warn!(
			"Remote room version {room_version_id} for {room_id} is not supported by conduwuit",
		)));
	}

	let mut leave_event_stub = serde_json::from_str::<CanonicalJsonObject>(
		make_leave_response.event.get(),
	)
	.map_err(|e| {
		err!(BadServerResponse(warn!(
			"Invalid make_leave event json received from {remote_server} for {room_id}: {e:?}"
		)))
	})?;

	// TODO: Is origin needed?
	leave_event_stub.insert(
		"origin".to_owned(),
		CanonicalJsonValue::String(
			self.services
				.globals
				.server_name()
				.as_str()
				.to_owned(),
		),
	);
	leave_event_stub.insert(
		"origin_server_ts".to_owned(),
		CanonicalJsonValue::Integer(
			utils::millis_since_unix_epoch()
				.try_into()
				.expect("Timestamp is valid js_int value"),
		),
	);

	// room v3 and above removed the "event_id" field from remote PDU format
	match room_version_id {
		| RoomVersionId::V1 | RoomVersionId::V2 => {},
		| _ => {
			leave_event_stub.remove("event_id");
		},
	}

	// In order to create a compatible ref hash (EventID) the `hashes` field needs
	// to be present
	self.services
		.server_keys
		.hash_and_sign_event(&mut leave_event_stub, &room_version_id)?;

	// Generate event id
	let event_id = gen_event_id(&leave_event_stub, &room_version_id)?;

	// Add event_id back
	leave_event_stub
		.insert("event_id".to_owned(), CanonicalJsonValue::String(event_id.clone().into()));

	// It has enough fields to be called a proper event now
	let leave_event = leave_event_stub;

	self.services
		.sending
		.send_federation_request(
			&remote_server,
			federation::membership::create_leave_event::v2::Request {
				room_id: room_id.to_owned(),
				event_id,
				pdu: self
					.services
					.federation
					.format_pdu_into(leave_event.clone(), Some(&room_version_id))
					.await,
			},
		)
		.await?;

	Ok(())
}
