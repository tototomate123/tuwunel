use std::{
	ops::Range,
	time::{Duration, Instant},
};

use ruma::{CanonicalJsonObject, EventId, MilliSecondsSinceUnixEpoch, RoomId, ServerName};
use tuwunel_core::{
	Err, Result, debug,
	debug::INFO_SPAN_LEVEL,
	defer, implement,
	matrix::{Event, PduEvent},
};

#[implement(super::Service)]
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
	name = "prev",
	level = INFO_SPAN_LEVEL,
	skip_all,
	fields(%prev_id),
)]
pub(super) async fn handle_prev_pdu<'a, Pdu>(
	&self,
	origin: &'a ServerName,
	event_id: &'a EventId,
	room_id: &'a RoomId,
	eventid_info: Option<(PduEvent, CanonicalJsonObject)>,
	create_event: &'a Pdu,
	first_ts_in_room: MilliSecondsSinceUnixEpoch,
	prev_id: &'a EventId,
) -> Result
where
	Pdu: Event,
{
	// Check for disabled again because it might have changed
	if self.services.metadata.is_disabled(room_id).await {
		return Err!(Request(Forbidden(debug_warn!(
			"Federaton of room {room_id} is currently disabled on this server. Request by \
			 origin {origin} and event ID {event_id}"
		))));
	}

	if self.is_backed_off(prev_id, Range {
		start: Duration::from_secs(5 * 60),
		end: Duration::from_secs(60 * 60 * 24),
	}) {
		debug!(?prev_id, "Backing off from prev_event");
		return Ok(());
	}

	let Some((pdu, json)) = eventid_info else {
		return Ok(());
	};

	// Skip old events
	if pdu.origin_server_ts() < first_ts_in_room {
		return Ok(());
	}

	let start_time = Instant::now();
	self.federation_handletime
		.write()
		.expect("locked")
		.insert(room_id.into(), (prev_id.to_owned(), start_time));

	defer! {{
		self.federation_handletime
			.write()
			.expect("locked")
			.remove(room_id);
	}};

	self.upgrade_outlier_to_timeline_pdu(pdu, json, create_event, origin, room_id)
		.await?;

	debug!(
		elapsed = ?start_time.elapsed(),
		"Handled prev_event",
	);

	Ok(())
}
