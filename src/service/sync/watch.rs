use futures::{FutureExt, StreamExt, pin_mut, stream::FuturesUnordered};
use ruma::{DeviceId, UserId};
use tuwunel_core::{Result, implement, trace};
use tuwunel_database::{Interfix, Separator, serialize_key};

#[implement(super::Service)]
#[tracing::instrument(skip(self), level = "debug")]
pub async fn watch(&self, user_id: &UserId, device_id: &DeviceId) -> Result {
	let userdeviceid_prefix = (user_id, device_id, Interfix);
	let globaluserdata_prefix = (Separator, user_id, Interfix);
	let userid_prefix =
		serialize_key((user_id, Interfix)).expect("failed to serialize watch prefix");

	let watchers = [
		// Return when *any* user changed their key
		// TODO: only send for user they share a room with
		self.db
			.todeviceid_events
			.watch_prefix(&userdeviceid_prefix)
			.boxed(),
		self.db
			.userroomid_joined
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		self.db
			.userroomid_invitestate
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		self.db
			.userroomid_leftstate
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		self.db
			.userroomid_knockedstate
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		self.db
			.userroomid_notificationcount
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		self.db
			.userroomid_highlightcount
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		self.db
			.roomusertype_roomuserdataid
			.watch_prefix(&globaluserdata_prefix)
			.boxed(),
		// More key changes (used when user is not joined to any rooms)
		self.db
			.keychangeid_userid
			.watch_raw_prefix(&userid_prefix)
			.boxed(),
		// One time keys
		self.db
			.userid_lastonetimekeyupdate
			.watch_raw_prefix(&user_id)
			.boxed(),
	];

	let mut futures = FuturesUnordered::new();
	futures.extend(watchers.into_iter());

	// Events for rooms we are in
	let rooms_joined = self.services.state_cache.rooms_joined(user_id);

	pin_mut!(rooms_joined);
	while let Some(room_id) = rooms_joined.next().await {
		let Ok(short_roomid) = self.services.short.get_shortroomid(room_id).await else {
			continue;
		};

		let roomid_prefix = (room_id, Interfix);
		let roomuser_prefix = (room_id, user_id);
		let typing_room_id = room_id.to_owned();
		let watchers = [
			// Key changes
			self.db
				.keychangeid_userid
				.watch_prefix(&roomid_prefix)
				.boxed(),
			// Room account data
			self.db
				.roomusertype_roomuserdataid
				.watch_prefix(&roomuser_prefix)
				.boxed(),
			// PDUs
			self.db
				.pduid_pdu
				.watch_prefix(&short_roomid)
				.boxed(),
			// EDUs
			self.db
				.readreceiptid_readreceipt
				.watch_prefix(&roomid_prefix)
				.boxed(),
			// Typing
			async move {
				self.services
					.typing
					.wait_for_update(&typing_room_id)
					.await;
			}
			.boxed(),
		];

		futures.extend(watchers.into_iter());
	}

	// Server shutdown
	futures.push(self.services.server.until_shutdown().boxed());

	if !self.services.server.running() {
		return Ok(());
	}

	// Wait until one of them finds something
	trace!(futures = futures.len(), "watch started");
	futures.next().await;
	trace!(futures = futures.len(), "watch finished");

	Ok(())
}
