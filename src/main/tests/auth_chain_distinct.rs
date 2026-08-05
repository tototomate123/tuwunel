#![cfg(test)]

use std::{env::var, fs::remove_dir_all, path::PathBuf, process::id as process_id};

use serde_json::json;
use tuwunel::{Args, Runtime, Server, async_run, async_start, async_stop};
use tuwunel_core::{
	Result,
	ruma::{CanonicalJsonObject, EventId, RoomId, RoomVersionId, event_id, room_id},
	utils::stream::TryReadyExt,
};
use tuwunel_service::Services;

const AUTH_CHAIN_BUCKETS: u64 = 50;

struct DatabasePath(PathBuf);

impl Drop for DatabasePath {
	fn drop(&mut self) { remove_dir_all(&self.0).ok(); }
}

#[test]
fn event_ids_iter_is_distinct_for_auth_diamond() -> Result {
	let root = var("TMPDIR").unwrap_or_else(|_| "/nvme/target/tmp".into());
	let db_path = DatabasePath(
		PathBuf::from(root).join(format!("tuwunel-auth-chain-distinct-{}", process_id())),
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
	let room_id = room_id!("!auth-chain-distinct:localhost");
	let left = event_id!("$left:localhost");
	let right = event_id!("$right:localhost");
	let tail = event_id!("$tail:localhost");

	add_outlier(services, room_id, tail, &[])?;
	add_outlier(services, room_id, left, &[tail])?;
	add_outlier(services, room_id, right, &[tail])?;

	let left_short = services
		.short
		.get_or_create_shorteventid(left)
		.await;

	let right_short = services
		.short
		.get_or_create_shorteventid(right)
		.await;

	// Separate buckets allow the convergent auth-chain walks to overlap.
	assert_ne!(left_short % AUTH_CHAIN_BUCKETS, right_short % AUTH_CHAIN_BUCKETS);

	let room_version = RoomVersionId::V6;
	let chain: Vec<_> = services
		.auth_chain
		.event_ids_iter(room_id, &room_version, [left, right].into_iter())
		.ready_try_fold(Vec::new(), |mut chain, event_id| {
			chain.push(event_id);

			Ok(chain)
		})
		.await?;

	assert_eq!(chain, [tail.to_owned()]);

	Ok(())
}

fn add_outlier(
	services: &Services,
	room_id: &RoomId,
	event_id: &EventId,
	auth_events: &[&EventId],
) -> Result {
	let pdu = json!({
		"auth_events": auth_events,
		"room_id": room_id,
	});
	let pdu: CanonicalJsonObject = serde_json::from_value(pdu)?;

	services.timeline.add_pdu_outlier(event_id, &pdu);

	Ok(())
}
