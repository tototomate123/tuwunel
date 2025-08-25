use axum::extract::State;
use axum_client_ip::InsecureClientIp;
use futures::FutureExt;
use ruma::{
	RoomId, RoomOrAliasId,
	api::client::membership::{join_room_by_id, join_room_by_id_or_alias},
};
use tuwunel_core::Result;

use super::banned_room_check;
use crate::{Ruma, client::membership::get_join_params};

/// # `POST /_matrix/client/r0/rooms/{roomId}/join`
///
/// Tries to join the sender user into a room.
///
/// - If the server knowns about this room: creates the join event and does auth
///   rules locally
/// - If the server does not know about the room: asks other servers over
///   federation
#[tracing::instrument(skip_all, fields(%client), name = "join")]
pub(crate) async fn join_room_by_id_route(
	State(services): State<crate::State>,
	InsecureClientIp(client): InsecureClientIp,
	body: Ruma<join_room_by_id::v3::Request>,
) -> Result<join_room_by_id::v3::Response> {
	let sender_user = body.sender_user();

	let room_id: &RoomId = &body.room_id;

	banned_room_check(&services, sender_user, Some(room_id), room_id.server_name(), client)
		.await?;

	let (room_id, servers) =
		get_join_params(&services, sender_user, <&RoomOrAliasId>::from(room_id), &[]).await?;

	let state_lock = services.state.mutex.lock(&room_id).await;

	services
		.membership
		.join(
			sender_user,
			&room_id,
			body.reason.clone(),
			&servers,
			&body.appservice_info,
			&state_lock,
		)
		.boxed()
		.await?;

	drop(state_lock);

	Ok(join_room_by_id::v3::Response { room_id })
}

/// # `POST /_matrix/client/r0/join/{roomIdOrAlias}`
///
/// Tries to join the sender user into a room.
///
/// - If the server knowns about this room: creates the join event and does auth
///   rules locally
/// - If the server does not know about the room: use the server name query
///   param if specified. if not specified, asks other servers over federation
///   via room alias server name and room ID server name
#[tracing::instrument(skip_all, fields(%client), name = "join")]
pub(crate) async fn join_room_by_id_or_alias_route(
	State(services): State<crate::State>,
	InsecureClientIp(client): InsecureClientIp,
	body: Ruma<join_room_by_id_or_alias::v3::Request>,
) -> Result<join_room_by_id_or_alias::v3::Response> {
	let sender_user = body.sender_user();
	let appservice_info = &body.appservice_info;

	let (room_id, servers) =
		get_join_params(&services, sender_user, &body.room_id_or_alias, &body.via).await?;

	banned_room_check(&services, sender_user, Some(&room_id), room_id.server_name(), client)
		.await?;

	let state_lock = services.state.mutex.lock(&room_id).await;

	services
		.membership
		.join(
			sender_user,
			&room_id,
			body.reason.clone(),
			&servers,
			appservice_info,
			&state_lock,
		)
		.boxed()
		.await?;

	drop(state_lock);

	Ok(join_room_by_id_or_alias::v3::Response { room_id })
}
