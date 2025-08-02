mod acl_check;
mod fetch_and_handle_outliers;
mod fetch_prev;
mod fetch_state;
mod handle_incoming_pdu;
mod handle_outlier_pdu;
mod handle_prev_pdu;
mod parse_incoming_pdu;
mod resolve_state;
mod state_at_incoming;
mod upgrade_outlier_pdu;

use std::{
	collections::hash_map,
	fmt::Write,
	ops::Range,
	sync::Arc,
	time::{Duration, Instant},
};

use async_trait::async_trait;
use ruma::{
	EventId, OwnedEventId, OwnedRoomId, RoomId, RoomVersionId,
	events::room::create::RoomCreateEventContent,
};
use tuwunel_core::{
	Err, Result, RoomVersion, Server, implement,
	matrix::{Event, PduEvent},
	utils::{MutexMap, continue_exponential_backoff},
};

use crate::{Dep, globals, rooms, sending, server_keys};

pub struct Service {
	pub mutex_federation: RoomMutexMap,
	services: Services,
}

struct Services {
	globals: Dep<globals::Service>,
	sending: Dep<sending::Service>,
	auth_chain: Dep<rooms::auth_chain::Service>,
	metadata: Dep<rooms::metadata::Service>,
	pdu_metadata: Dep<rooms::pdu_metadata::Service>,
	server_keys: Dep<server_keys::Service>,
	short: Dep<rooms::short::Service>,
	state: Dep<rooms::state::Service>,
	state_accessor: Dep<rooms::state_accessor::Service>,
	state_compressor: Dep<rooms::state_compressor::Service>,
	timeline: Dep<rooms::timeline::Service>,
	server: Arc<Server>,
}

type RoomMutexMap = MutexMap<OwnedRoomId, ()>;

#[async_trait]
impl crate::Service for Service {
	fn build(args: crate::Args<'_>) -> Result<Arc<Self>> {
		Ok(Arc::new(Self {
			mutex_federation: RoomMutexMap::new(),
			services: Services {
				globals: args.depend::<globals::Service>("globals"),
				sending: args.depend::<sending::Service>("sending"),
				auth_chain: args.depend::<rooms::auth_chain::Service>("rooms::auth_chain"),
				metadata: args.depend::<rooms::metadata::Service>("rooms::metadata"),
				server_keys: args.depend::<server_keys::Service>("server_keys"),
				pdu_metadata: args.depend::<rooms::pdu_metadata::Service>("rooms::pdu_metadata"),
				short: args.depend::<rooms::short::Service>("rooms::short"),
				state: args.depend::<rooms::state::Service>("rooms::state"),
				state_accessor: args
					.depend::<rooms::state_accessor::Service>("rooms::state_accessor"),
				state_compressor: args
					.depend::<rooms::state_compressor::Service>("rooms::state_compressor"),
				timeline: args.depend::<rooms::timeline::Service>("rooms::timeline"),
				server: args.server.clone(),
			},
		}))
	}

	async fn memory_usage(&self, out: &mut (dyn Write + Send)) -> Result {
		let mutex_federation = self.mutex_federation.len();
		writeln!(out, "federation_mutex: {mutex_federation}")?;

		Ok(())
	}

	fn name(&self) -> &str { crate::service::make_name(std::module_path!()) }
}

#[implement(Service)]
fn back_off(&self, event_id: &EventId) {
	use hash_map::Entry::{Occupied, Vacant};

	match self
		.services
		.globals
		.bad_event_ratelimiter
		.write()
		.expect("locked")
		.entry(event_id.into())
	{
		| Vacant(e) => {
			e.insert((Instant::now(), 1));
		},
		| Occupied(mut e) => {
			*e.get_mut() = (Instant::now(), e.get().1.saturating_add(1));
		},
	}
}

#[implement(Service)]
fn is_backed_off(&self, event_id: &EventId, range: Range<Duration>) -> bool {
	let Some((time, tries)) = self
		.services
		.globals
		.bad_event_ratelimiter
		.read()
		.expect("locked")
		.get(event_id)
		.copied()
	else {
		return false;
	};

	continue_exponential_backoff(range.start, range.end, time.elapsed(), tries)
}

#[implement(Service)]
async fn event_exists(&self, event_id: OwnedEventId) -> bool {
	self.services.timeline.pdu_exists(&event_id).await
}

#[implement(Service)]
async fn event_fetch(&self, event_id: OwnedEventId) -> Option<PduEvent> {
	self.services
		.timeline
		.get_pdu(&event_id)
		.await
		.ok()
}

fn check_room_id<Pdu: Event>(room_id: &RoomId, pdu: &Pdu) -> Result {
	if pdu.room_id() != room_id {
		return Err!(Request(InvalidParam(error!(
			pdu_event_id = ?pdu.event_id(),
			pdu_room_id = ?pdu.room_id(),
			?room_id,
			"Found event from room in room",
		))));
	}

	Ok(())
}

fn get_room_version_id<Pdu: Event>(create_event: &Pdu) -> Result<RoomVersionId> {
	let content: RoomCreateEventContent = create_event.get_content()?;
	let room_version = content.room_version;

	Ok(room_version)
}

#[inline]
fn to_room_version(room_version_id: &RoomVersionId) -> RoomVersion {
	RoomVersion::new(room_version_id).expect("room version is supported")
}
