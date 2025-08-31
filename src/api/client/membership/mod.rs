mod ban;
mod forget;
mod invite;
mod join;
mod kick;
mod knock;
mod leave;
mod members;
mod unban;

use std::net::IpAddr;

use axum::extract::State;
use futures::{FutureExt, StreamExt};
use ruma::{
	OwnedRoomId, OwnedServerName, RoomId, RoomOrAliasId, ServerName, UserId,
	api::client::membership::joined_rooms,
};
use tuwunel_core::{Err, Result, result::LogErr, utils::shuffle, warn};
use tuwunel_service::Services;

pub(crate) use self::{
	ban::ban_user_route,
	forget::forget_room_route,
	invite::invite_user_route,
	join::{join_room_by_id_or_alias_route, join_room_by_id_route},
	kick::kick_user_route,
	knock::knock_room_route,
	leave::leave_room_route,
	members::{get_member_events_route, joined_members_route},
	unban::unban_user_route,
};
use crate::Ruma;

/// # `POST /_matrix/client/r0/joined_rooms`
///
/// Lists all rooms the user has joined.
pub(crate) async fn joined_rooms_route(
	State(services): State<crate::State>,
	body: Ruma<joined_rooms::v3::Request>,
) -> Result<joined_rooms::v3::Response> {
	Ok(joined_rooms::v3::Response {
		joined_rooms: services
			.state_cache
			.rooms_joined(body.sender_user())
			.map(ToOwned::to_owned)
			.collect()
			.await,
	})
}

/// Checks if the room is banned in any way possible and the sender user is not
/// an admin.
///
/// Performs automatic deactivation if `auto_deactivate_banned_room_attempts` is
/// enabled
#[tracing::instrument(skip(services))]
pub(crate) async fn banned_room_check(
	services: &Services,
	user_id: &UserId,
	room_id: Option<&RoomId>,
	server_name: Option<&ServerName>,
	client_ip: IpAddr,
) -> Result {
	if services.users.is_admin(user_id).await {
		return Ok(());
	}

	// TODO: weird condition
	if let Some(room_id) = room_id {
		if services.metadata.is_banned(room_id).await
			|| (room_id.server_name().is_some()
				&& services
					.config
					.forbidden_remote_server_names
					.is_match(
						room_id
							.server_name()
							.expect("legacy room mxid")
							.host(),
					)) {
			warn!(
				"User {user_id} who is not an admin attempted to send an invite for or \
				 attempted to join a banned room or banned room server name: {room_id}"
			);

			maybe_deactivate(services, user_id, client_ip)
				.await
				.log_err()
				.ok();

			return Err!(Request(Forbidden("This room is banned on this homeserver.")));
		}
	} else if let Some(server_name) = server_name {
		if services
			.config
			.forbidden_remote_server_names
			.is_match(server_name.host())
		{
			warn!(
				"User {user_id} who is not an admin tried joining a room which has the server \
				 name {server_name} that is globally forbidden. Rejecting.",
			);

			maybe_deactivate(services, user_id, client_ip)
				.await
				.log_err()
				.ok();

			return Err!(Request(Forbidden("This remote server is banned on this homeserver.")));
		}
	}

	Ok(())
}

async fn maybe_deactivate(services: &Services, user_id: &UserId, client_ip: IpAddr) -> Result {
	if services
		.server
		.config
		.auto_deactivate_banned_room_attempts
	{
		warn!("Automatically deactivating user {user_id} due to attempted banned room join");

		if services.server.config.admin_room_notices {
			services
				.admin
				.send_text(&format!(
					"Automatically deactivating user {user_id} due to attempted banned room \
					 join from IP {client_ip}"
				))
				.await;
		}

		services
			.deactivate
			.full_deactivate(user_id)
			.boxed()
			.await?;
	}

	Ok(())
}

// TODO: should this be in services? banned check would have to resolve again if
// room_id is not available at callsite
async fn get_join_params(
	services: &Services,
	user_id: &UserId,
	room_id_or_alias: &RoomOrAliasId,
	via: &[OwnedServerName],
) -> Result<(OwnedRoomId, Vec<OwnedServerName>)> {
	// servers tried first, additional_servers shuffled then tried after
	let (room_id, mut servers, mut additional_servers) =
		match OwnedRoomId::try_from(room_id_or_alias.to_owned()) {
			// if room id, shuffle via + room_id server_name ...
			| Ok(room_id) => {
				let mut additional_servers = via.to_vec();

				if let Some(server) = room_id.server_name() {
					additional_servers.push(server.to_owned());
				}

				(room_id, Vec::new(), additional_servers)
			},
			// ... if room alias, resolve and don't shuffle ...
			| Err(room_alias) => {
				let (room_id, servers) = services
					.alias
					.resolve_alias(&room_alias, Some(via.to_vec()))
					.await?;

				(room_id, servers, Vec::new())
			},
		};

	// either way, add invited vias
	additional_servers.extend(
		services
			.state_cache
			.servers_invite_via(&room_id)
			.map(ToOwned::to_owned)
			.collect::<Vec<_>>()
			.await,
	);

	// either way, add invite senders' servers
	additional_servers.extend(
		services
			.state_cache
			.invite_state(user_id, &room_id)
			.await
			.unwrap_or_default()
			.iter()
			.filter_map(|event| event.get_field("sender").ok().flatten())
			.filter_map(|sender: &str| UserId::parse(sender).ok())
			.map(|user| user.server_name().to_owned()),
	);

	// shuffle additionals, append to base servers
	additional_servers.sort_unstable();
	additional_servers.dedup();
	shuffle(&mut additional_servers);
	servers.sort_unstable();
	servers.dedup();
	servers.append(&mut additional_servers);

	Ok((room_id, servers))
}
