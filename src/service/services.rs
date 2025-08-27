use std::sync::Arc;

use futures::{StreamExt, TryStreamExt};
use tokio::sync::Mutex;
use tuwunel_core::{
	Result, Server, debug, debug_info, implement, info, trace, utils::stream::IterStream,
};
use tuwunel_database::Database;

pub(crate) use crate::OnceServices;
use crate::{
	account_data, admin, appservice, client, config, deactivate, emergency, federation, globals,
	key_backups,
	manager::Manager,
	media, membership, presence, pusher, resolver, rooms, sending, server_keys,
	service::{Args, Service},
	sync, transaction_ids, uiaa, users,
};

pub struct Services {
	pub account_data: Arc<account_data::Service>,
	pub admin: Arc<admin::Service>,
	pub appservice: Arc<appservice::Service>,
	pub config: Arc<config::Service>,
	pub client: Arc<client::Service>,
	pub emergency: Arc<emergency::Service>,
	pub globals: Arc<globals::Service>,
	pub key_backups: Arc<key_backups::Service>,
	pub media: Arc<media::Service>,
	pub presence: Arc<presence::Service>,
	pub pusher: Arc<pusher::Service>,
	pub resolver: Arc<resolver::Service>,
	pub alias: Arc<rooms::alias::Service>,
	pub auth_chain: Arc<rooms::auth_chain::Service>,
	pub delete: Arc<rooms::delete::Service>,
	pub directory: Arc<rooms::directory::Service>,
	pub event_handler: Arc<rooms::event_handler::Service>,
	pub lazy_loading: Arc<rooms::lazy_loading::Service>,
	pub metadata: Arc<rooms::metadata::Service>,
	pub pdu_metadata: Arc<rooms::pdu_metadata::Service>,
	pub read_receipt: Arc<rooms::read_receipt::Service>,
	pub search: Arc<rooms::search::Service>,
	pub short: Arc<rooms::short::Service>,
	pub spaces: Arc<rooms::spaces::Service>,
	pub state: Arc<rooms::state::Service>,
	pub state_accessor: Arc<rooms::state_accessor::Service>,
	pub state_cache: Arc<rooms::state_cache::Service>,
	pub state_compressor: Arc<rooms::state_compressor::Service>,
	pub threads: Arc<rooms::threads::Service>,
	pub timeline: Arc<rooms::timeline::Service>,
	pub typing: Arc<rooms::typing::Service>,
	pub user: Arc<rooms::user::Service>,
	pub federation: Arc<federation::Service>,
	pub sending: Arc<sending::Service>,
	pub server_keys: Arc<server_keys::Service>,
	pub sync: Arc<sync::Service>,
	pub transaction_ids: Arc<transaction_ids::Service>,
	pub uiaa: Arc<uiaa::Service>,
	pub users: Arc<users::Service>,
	pub membership: Arc<membership::Service>,
	pub deactivate: Arc<deactivate::Service>,

	manager: Mutex<Option<Arc<Manager>>>,
	pub server: Arc<Server>,
	pub db: Arc<Database>,
}

#[implement(Services)]
pub async fn build(server: Arc<Server>) -> Result<Arc<Self>> {
	let db = Database::open(&server).await?;
	let services = Arc::new(OnceServices::default());
	macro_rules! build {
		($tyname:ty) => {
			<$tyname>::build(Args {
				db: &db,
				server: &server,
				services: &services,
			})?
		};
	}

	let res = Arc::new(Self {
		account_data: build!(account_data::Service),
		admin: build!(admin::Service),
		appservice: build!(appservice::Service),
		resolver: build!(resolver::Service),
		client: build!(client::Service),
		config: build!(config::Service),
		emergency: build!(emergency::Service),
		globals: build!(globals::Service),
		key_backups: build!(key_backups::Service),
		media: build!(media::Service),
		presence: build!(presence::Service),
		pusher: build!(pusher::Service),
		alias: build!(rooms::alias::Service),
		auth_chain: build!(rooms::auth_chain::Service),
		delete: build!(rooms::delete::Service),
		directory: build!(rooms::directory::Service),
		event_handler: build!(rooms::event_handler::Service),
		lazy_loading: build!(rooms::lazy_loading::Service),
		metadata: build!(rooms::metadata::Service),
		pdu_metadata: build!(rooms::pdu_metadata::Service),
		read_receipt: build!(rooms::read_receipt::Service),
		search: build!(rooms::search::Service),
		short: build!(rooms::short::Service),
		spaces: build!(rooms::spaces::Service),
		state: build!(rooms::state::Service),
		state_accessor: build!(rooms::state_accessor::Service),
		state_cache: build!(rooms::state_cache::Service),
		state_compressor: build!(rooms::state_compressor::Service),
		threads: build!(rooms::threads::Service),
		timeline: build!(rooms::timeline::Service),
		typing: build!(rooms::typing::Service),
		user: build!(rooms::user::Service),
		federation: build!(federation::Service),
		sending: build!(sending::Service),
		server_keys: build!(server_keys::Service),
		sync: build!(sync::Service),
		transaction_ids: build!(transaction_ids::Service),
		uiaa: build!(uiaa::Service),
		users: build!(users::Service),
		membership: build!(membership::Service),
		deactivate: build!(deactivate::Service),

		manager: Mutex::new(None),
		server,
		db,
	});

	Ok(services.set(res))
}

#[implement(Services)]
pub(crate) fn services(&self) -> impl Iterator<Item = Arc<dyn Service>> + Send {
	macro_rules! cast {
		($s:expr) => {
			<Arc<dyn Service> as Into<_>>::into($s.clone())
		};
	}

	[
		cast!(self.account_data),
		cast!(self.admin),
		cast!(self.appservice),
		cast!(self.resolver),
		cast!(self.client),
		cast!(self.config),
		cast!(self.emergency),
		cast!(self.globals),
		cast!(self.key_backups),
		cast!(self.media),
		cast!(self.presence),
		cast!(self.pusher),
		cast!(self.alias),
		cast!(self.auth_chain),
		cast!(self.delete),
		cast!(self.directory),
		cast!(self.event_handler),
		cast!(self.lazy_loading),
		cast!(self.metadata),
		cast!(self.pdu_metadata),
		cast!(self.read_receipt),
		cast!(self.search),
		cast!(self.short),
		cast!(self.spaces),
		cast!(self.state),
		cast!(self.state_accessor),
		cast!(self.state_cache),
		cast!(self.state_compressor),
		cast!(self.threads),
		cast!(self.timeline),
		cast!(self.typing),
		cast!(self.user),
		cast!(self.federation),
		cast!(self.sending),
		cast!(self.server_keys),
		cast!(self.sync),
		cast!(self.transaction_ids),
		cast!(self.uiaa),
		cast!(self.users),
		cast!(self.membership),
		cast!(self.deactivate),
	]
	.into_iter()
}

#[implement(Services)]
pub async fn start(self: &Arc<Self>) -> Result<Arc<Self>> {
	debug_info!("Starting services...");

	super::migrations::migrations(self).await?;
	self.manager
		.lock()
		.await
		.insert(Manager::new(self))
		.clone()
		.start()
		.await?;

	debug_info!("Services startup complete.");

	Ok(Arc::clone(self))
}

#[implement(Services)]
pub async fn stop(&self) {
	info!("Shutting down services...");

	self.interrupt().await;
	if let Some(manager) = self.manager.lock().await.as_ref() {
		manager.stop().await;
	}

	debug_info!("Services shutdown complete.");
}

#[implement(Services)]
pub(crate) async fn interrupt(&self) {
	debug!("Interrupting services...");
	for service in self.services() {
		let name = service.name();
		trace!("Interrupting {name}");
		service.interrupt().await;
	}
}

#[implement(Services)]
pub async fn poll(&self) -> Result {
	if let Some(manager) = self.manager.lock().await.as_ref() {
		return manager.poll().await;
	}

	Ok(())
}

#[implement(Services)]
pub async fn clear_cache(&self) {
	self.services()
		.stream()
		.for_each(async |service| {
			service.clear_cache().await;
		})
		.await;
}

#[implement(Services)]
pub async fn memory_usage(&self) -> Result<String> {
	self.services()
		.try_stream()
		.try_fold(String::new(), async |mut out, service| {
			service.memory_usage(&mut out).await?;
			Ok(out)
		})
		.await
}
