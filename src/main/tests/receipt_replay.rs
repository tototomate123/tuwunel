#![cfg(test)]

use std::{
	collections::BTreeMap, env::var, fs::remove_dir_all, path::PathBuf, process::id as process_id,
};

use tuwunel::{Args, Runtime, Server, async_run, async_start, async_stop};
use tuwunel_core::{
	Err, Result,
	ruma::{
		EventId, MilliSecondsSinceUnixEpoch, RoomId, UserId, event_id,
		events::receipt::{
			Receipt, ReceiptEvent, ReceiptEventContent, ReceiptThread, ReceiptType,
		},
		room_id, user_id,
	},
	utils::ReadyExt,
};
use tuwunel_service::Services;

struct DatabasePath(PathBuf);

impl Drop for DatabasePath {
	fn drop(&mut self) { remove_dir_all(&self.0).ok(); }
}

#[test]
fn replayed_receipts_hold_their_stream_position() -> Result {
	let root = var("TMPDIR").unwrap_or_else(|_| "/nvme/target/tmp".into());
	let db_path = DatabasePath(
		PathBuf::from(root).join(format!("tuwunel-receipt-replay-{}", process_id())),
	);

	let mut args = Args::default_test(&["fresh", "cleanup"]);
	args.maintenance = true;
	args.option
		.push(format!("database_path={:?}", db_path.0));

	let runtime = Runtime::new(Some(&args))?;
	let server = Server::new(Some(&args), Some(&runtime))?;
	let result = runtime.block_on(async {
		let services = async_start(&server).await?;
		let outcome = exercise(&services).await;
		let shutdown = server.server.shutdown();

		drop(services);

		let run = async_run(&server).await;
		let stop = async_stop(&server).await;

		outcome.and(shutdown).and(run).and(stop)
	});

	drop(runtime);

	result
}

async fn exercise(services: &Services) -> Result {
	let room = room_id!("!receipt-replay:localhost");
	let user = user_id!("@receipt-replay:localhost");
	let receipts = &services.read_receipt;
	let first = receipt_event(room, user, event_id!("$receipt-replay-first:localhost"));
	let second = receipt_event(room, user, event_id!("$receipt-replay-second:localhost"));

	let stored = receipts
		.readreceipt_update(user, room, &first)
		.await;

	let after_store = receipt_counts(services, room).await;
	let replayed = receipts
		.readreceipt_update(user, room, &first)
		.await;

	let after_replay = receipt_counts(services, room).await;
	let advanced = receipts
		.readreceipt_update(user, room, &second)
		.await;

	let after_advance = receipt_counts(services, room).await;

	if !stored || after_store.len() != 1 {
		return Err!("first receipt did not store exactly one row");
	}

	if replayed || after_replay != after_store {
		return Err!("replayed receipt moved the receipt stream");
	}

	if !advanced || after_advance.len() != 1 || after_advance[0] <= after_store[0] {
		return Err!("receipt naming another event did not take a new position");
	}

	Ok(())
}

fn receipt_event(room: &RoomId, user: &UserId, event: &EventId) -> ReceiptEvent {
	let receipt = Receipt {
		ts: Some(MilliSecondsSinceUnixEpoch::now()),
		thread: ReceiptThread::Unthreaded,
	};

	let users = BTreeMap::from([(user.to_owned(), receipt)]);
	let read = BTreeMap::from([(ReceiptType::Read, users)]);
	let content = ReceiptEventContent(BTreeMap::from([(event.to_owned(), read)]));

	ReceiptEvent { content, room_id: room.to_owned() }
}

async fn receipt_counts(services: &Services, room: &RoomId) -> Vec<u64> {
	services
		.read_receipt
		.readreceipts_since(room, 0, None)
		.ready_fold(Vec::new(), |mut counts, (_, count, _)| {
			counts.push(count);
			counts
		})
		.await
}
