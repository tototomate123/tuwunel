#![cfg(test)]

use std::{
	env::var, fs::remove_dir_all, net::TcpListener, path::PathBuf, process::id as process_id,
};

use serde_json::json;
use tuwunel::{Args, Runtime, Server, async_run, async_start, async_stop};
use tuwunel_core::{
	Err, Result,
	ruma::{EventId, RoomId, UserId},
	utils::BoolExt,
};
use tuwunel_service::Services;

use self::msc3664::{CONDITION_KIND, Client, highlighted, notified, register, wait_until_ready};

mod msc3664;

const AUTHOR_TOKEN: &str = "msc3664-related-event-match-author-token";
const READER_TOKEN: &str = "msc3664-related-event-match-reader-token";

/// Drives an MSC3664 push rule end to end over the client API.
///
/// The author writes an `im.nheko.msc3664.related_event_match` rule for
/// annotations of their own events, and a reaction from another user must then
/// notify them. A reaction to somebody else's event exercises the same rule and
/// must not, which is what shows the condition reads the related event's sender
/// rather than merely the relation type.
///
/// The paired `msc3664_disabled` binary holds the other half, that the same
/// rule shape matches nothing while the option is unset.
#[test]
fn reaction_to_my_message_notifies_me() -> Result {
	let listener = TcpListener::bind(("127.0.0.1", 0))?;
	let port = listener.local_addr()?.port();

	let root = var("TMPDIR").unwrap_or_else(|_| "/nvme/target/tmp".into());
	let db_path = PathBuf::from(root).join(format!("tuwunel-msc3664-reaction-{}", process_id()));

	let mut args = Args::default_test(&["fresh", "cleanup"]);

	args.option.extend([
		format!("database_path={db_path:?}"),
		"address=[\"127.0.0.1\"]".to_owned(),
		format!("port={port}"),
		"listening=true".to_owned(),
		"msc3664_related_event_match=true".to_owned(),
	]);

	let runtime = Runtime::new(Some(&args))?;
	let server = Server::new(Some(&args), Some(&runtime))?;
	let result = runtime.block_on(async {
		let services = async_start(&server).await?;
		let base = format!("http://127.0.0.1:{port}");

		drop(listener);

		let exercise = async {
			let outcome = exercise(&services, &base).await;
			let shutdown = server.server.shutdown();

			outcome.and(shutdown)
		};

		let (run_result, outcome) = tokio::join!(async_run(&server), exercise);

		drop(services);
		async_stop(&server).await?;
		run_result?;

		outcome
	});

	drop(runtime);
	remove_dir_all(&db_path).ok();

	result
}

async fn exercise(services: &Services, base: &str) -> Result {
	wait_until_ready(services, base).await?;

	let author = register(services, "msc3664author", AUTHOR_TOKEN).await?;

	register(services, "msc3664reader", READER_TOKEN).await?;

	let writer = Client { services, base, token: AUTHOR_TOKEN };
	let reader = Client { services, base, token: READER_TOKEN };
	let room = writer.create_room().await?;

	reader.join(&room).await?;

	if writer.condition_capability().await?.is_none() {
		return Err!("the condition is evaluated but its capability is not advertised");
	}

	let rule = json!({
		"conditions": [{
			"kind": CONDITION_KIND,
			"rel_type": "m.annotation",
			"key": "sender",
			"pattern": author.as_str(),
		}],
		"actions": ["notify"],
	});

	writer
		.set_push_rule("msc3664_reactions_to_me", &rule)
		.await?;

	let mine = writer
		.send(
			&room,
			"m.room.message",
			"t1",
			&json!({
				"msgtype": "m.text",
				"body": "Dinner at 7?",
			}),
		)
		.await?;

	// The author sent it, so nothing so far has notified them.
	let before = services
		.pusher
		.notification_count(&author, &room)
		.await;

	reader
		.send(
			&room,
			"m.reaction",
			"t2",
			&json!({
				"m.relates_to": {
					"rel_type": "m.annotation",
					"event_id": mine,
					"key": "👍",
				},
			}),
		)
		.await?;

	if notified(services, &author, &room, before.saturating_add(1))
		.await
		.is_false()
	{
		return Err!("a reaction to the author's own message did not notify them");
	}

	// Somebody else's message, which notifies the author on its own. Letting
	// this one lag would sample the baseline low and fail the case below.
	let theirs = reader
		.send(
			&room,
			"m.room.message",
			"t3",
			&json!({
				"msgtype": "m.text",
				"body": "Works for me",
			}),
		)
		.await?;

	if notified(services, &author, &room, before.saturating_add(2))
		.await
		.is_false()
	{
		return Err!("the other user's message did not notify the author");
	}

	let baseline = services
		.pusher
		.notification_count(&author, &room)
		.await;

	reader
		.send(
			&room,
			"m.reaction",
			"t4",
			&json!({
				"m.relates_to": {
					"rel_type": "m.annotation",
					"event_id": theirs,
					"key": "👍",
				},
			}),
		)
		.await?;

	// The same rule sees a relation whose target the author did not send, so it
	// must fall through to the default rule suppressing reactions.
	if notified(services, &author, &room, baseline.saturating_add(1)).await {
		return Err!("a reaction to another user's message notified the author");
	}

	replies_notify(services, &writer, &reader, &author, &room, &mine).await?;
	cross_room_relations_are_not_followed(services, &writer, &reader, &author, &room).await
}

/// A reply is matched as a relation of its own type, and a thread's reply
/// fallback is not a reply.
///
/// The rule asks for the highlight tweak because the default rule notifying for
/// any message would otherwise satisfy an assertion on the notification count.
async fn replies_notify(
	services: &Services,
	writer: &Client<'_>,
	reader: &Client<'_>,
	author: &UserId,
	room: &RoomId,
	mine: &EventId,
) -> Result {
	let rule = json!({
		"conditions": [{
			"kind": CONDITION_KIND,
			"rel_type": "m.in_reply_to",
			"key": "sender",
			"pattern": author.as_str(),
		}],
		"actions": ["notify", { "set_tweak": "highlight" }],
	});

	writer
		.set_push_rule("msc3664_replies_to_me", &rule)
		.await?;

	let before = services
		.pusher
		.highlight_count(author, room)
		.await;

	reader
		.send(
			room,
			"m.room.message",
			"t5",
			&json!({
				"msgtype": "m.text",
				"body": "Sounds good",
				"m.relates_to": { "m.in_reply_to": { "event_id": mine } },
			}),
		)
		.await?;

	if highlighted(services, author, room, before.saturating_add(1))
		.await
		.is_false()
	{
		return Err!("a reply to the author's message did not highlight");
	}

	let baseline = services
		.pusher
		.highlight_count(author, room)
		.await;

	reader
		.send(
			room,
			"m.room.message",
			"t6",
			&json!({
				"msgtype": "m.text",
				"body": "In the thread",
				"m.relates_to": {
					"rel_type": "m.thread",
					"event_id": mine,
					"is_falling_back": true,
					"m.in_reply_to": { "event_id": mine },
				},
			}),
		)
		.await?;

	if highlighted(services, author, room, baseline.saturating_add(1)).await {
		return Err!("a thread's reply fallback was matched as a reply");
	}

	Ok(())
}

/// A relation naming an event in another room is not followed.
///
/// The target is one the author sent, so the rule would match it on sender
/// alone; only the room check keeps the condition from reading an event the
/// reacting user need not be able to see.
async fn cross_room_relations_are_not_followed(
	services: &Services,
	writer: &Client<'_>,
	reader: &Client<'_>,
	author: &UserId,
	room: &RoomId,
) -> Result {
	let elsewhere = writer.create_room().await?;
	let hidden = writer
		.send(
			&elsewhere,
			"m.room.message",
			"t7",
			&json!({
				"msgtype": "m.text",
				"body": "Somewhere the reader is not",
			}),
		)
		.await?;

	let baseline = services
		.pusher
		.notification_count(author, room)
		.await;

	reader
		.send(
			room,
			"m.reaction",
			"t8",
			&json!({
				"m.relates_to": {
					"rel_type": "m.annotation",
					"event_id": hidden,
					"key": "👍",
				},
			}),
		)
		.await?;

	if notified(services, author, room, baseline.saturating_add(1)).await {
		return Err!("a relation into another room was followed");
	}

	Ok(())
}
