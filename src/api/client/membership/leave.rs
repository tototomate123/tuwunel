use axum::extract::State;
use futures::FutureExt;
use ruma::api::client::membership::leave_room;
use tuwunel_core::Result;

use crate::Ruma;

/// # `POST /_matrix/client/v3/rooms/{roomId}/leave`
///
/// Tries to leave the sender user from a room.
///
/// - This should always work if the user is currently joined.
pub(crate) async fn leave_room_route(
	State(services): State<crate::State>,
	body: Ruma<leave_room::v3::Request>,
) -> Result<leave_room::v3::Response> {
	let room_id = &body.room_id;

	let state_lock = services.state.mutex.lock(room_id).await;

	services
		.membership
		.leave(body.sender_user(), room_id, body.reason.clone(), &state_lock)
		.boxed()
		.await?;

	drop(state_lock);

	Ok(leave_room::v3::Response {})
}
