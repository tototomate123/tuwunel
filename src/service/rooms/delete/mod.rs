use std::{sync::Arc, time::Duration};

use futures::{FutureExt, StreamExt, pin_mut};
use ruma::RoomId;
use tokio::time::sleep;
use tuwunel_core::{
	Result, debug,
	result::LogErr,
	trace,
	utils::{ReadyExt, future::BoolExt},
	warn,
};

use crate::rooms::timeline::RoomMutexGuard;

pub struct Service {
	services: Arc<crate::services::OnceServices>,
}

impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self { services: args.services.clone() }))
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

impl Service {
	pub async fn delete_if_empty_local(&self, room_id: &RoomId, state_lock: RoomMutexGuard) {
		debug_assert!(
			self.services.config.delete_rooms_after_leave,
			"Caller must checking if delete_rooms_after_leave configured."
		);

		let has_local_users = self
			.services
			.state_cache
			.local_users_in_room(room_id)
			.into_future()
			.map(|(next, ..)| next.as_ref().is_some());

		let has_local_invites = self
			.services
			.state_cache
			.local_users_invited_to_room(room_id)
			.into_future()
			.map(|(next, ..)| next.as_ref().is_some());

		pin_mut!(has_local_users, has_local_invites);
		if has_local_users.or(has_local_invites).await {
			trace!(?room_id, "Not deleting with local joined or invited");
			return;
		}

		debug!(?room_id, "Preparing to delete room...");

		// Some arbitrary delay has to account for the leave event being synced to the
		// client or they'll never be updated on their leave. This can be removed once
		// a tombstone solution is implemented instead.
		sleep(Duration::from_millis(2500)).await;

		self.services
			.delete
			.delete_room(room_id, state_lock)
			.boxed()
			.await
			.expect("unhandled error during room deletion");
	}

	pub async fn delete_room(&self, room_id: &RoomId, state_lock: RoomMutexGuard) -> Result {
		// ban the room locally so new users cannot join while we're in the process of
		// deleting it
		debug!("Banning room {room_id} prior to deletion.");
		self.services.metadata.ban_room(room_id);

		// This might have to be dropped here to prevent deadlock, but the goal should
		// be to hold it all the way through. For now the room is banned under lock at
		// least.
		drop(state_lock);

		debug!("Making all users leave the room {room_id} and forgetting it");
		let mut users = self
			.services
			.state_cache
			.room_members(room_id)
			.ready_filter(|user| self.services.globals.user_is_local(user))
			.boxed();

		while let Some(user_id) = users.next().await {
			debug!(
				"Attempting leave for user {user_id} in room {room_id} (ignoring all errors, \
				 evicting admins too)",
			);

			if let Err(e) = self
				.services
				.membership
				.remote_leave(user_id, room_id)
				.await
			{
				warn!("Failed to leave room: {e}");
			}

			self.services.state_cache.forget(room_id, user_id);
		}

		debug!("Disabling incoming federation on room {room_id}");
		self.services.metadata.disable_room(room_id);

		debug!("Deleting all our room aliases for the room");
		self.services
			.alias
			.local_aliases_for_room(room_id)
			.for_each(async |local_alias| {
				self.services
					.alias
					.remove_alias(local_alias, &self.services.globals.server_user)
					.await
					.log_err()
					.ok();
			})
			.await;

		debug!("Removing/unpublishing room from our room directory");
		self.services.directory.set_not_public(room_id);

		debug!("Deleting room's threads from database");
		self.services
			.threads
			.delete_all_rooms_threads(room_id)
			.await
			.log_err()
			.ok();

		debug!("Deleting all the room's search token IDs from our database");
		self.services
			.search
			.delete_all_search_tokenids_for_room(room_id)
			.await
			.log_err()
			.ok();

		debug!("Deleting all room's forward extremities from our database");
		self.services
			.state
			.delete_all_rooms_forward_extremities(room_id)
			.await
			.log_err()
			.ok();

		debug!("Deleting all the room's event (PDU) references");
		self.services
			.pdu_metadata
			.delete_all_referenced_for_room(room_id)
			.await
			.log_err()
			.ok();

		debug!("Deleting all the room's member counts");
		self.services
			.state_cache
			.delete_room_join_counts(room_id)
			.await
			.log_err()
			.ok();

		debug!("Deleting all the room's private read receipts");
		self.services
			.read_receipt
			.delete_all_read_receipts(room_id)
			.await
			.log_err()
			.ok();

		debug!("Final stages of deleting the room");

		debug!("Obtaining a mutex state lock for safety and future database operations");
		let state_lock = self.services.state.mutex.lock(room_id).await;

		debug!("Deleting room state hash from our database");
		self.services
			.state
			.delete_room_shortstatehash(room_id, &state_lock)
			.await
			.log_err()
			.ok();

		debug!("Deleting PDUs");
		self.services
			.timeline
			.delete_pdus(room_id)
			.await
			.log_err()
			.ok();

		debug!("Deleting internal room ID from our database");
		self.services
			.short
			.delete_shortroomid(room_id)
			.await
			.log_err()
			.ok();

		// TODO: add option to keep a room banned (`--block` or `--ban`)
		self.services.metadata.enable_room(room_id);
		self.services.metadata.unban_room(room_id);

		drop(state_lock);

		debug!("Successfully deleted room {room_id} from our database");
		Ok(())
	}
}
