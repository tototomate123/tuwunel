use std::{
	ops::Deref,
	sync::{Arc, OnceLock},
};

use futures::{StreamExt, TryStreamExt};
use tokio::sync::Mutex;
use tuwunel_core::{Result, Server, debug, debug_info, err, info, trace};
use tuwunel_database::Database;

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

pub struct OnceServices {
	lock: OnceLock<Arc<Services>>,
}

impl OnceServices {
	pub fn get_services(&self) -> &Arc<Services> {
		self.lock
			.get()
			.expect("services must be initialized")
	}
}

impl Deref for OnceServices {
	type Target = Arc<Services>;

	fn deref(&self) -> &Self::Target { self.get_services() }
}

impl Services {
	#[allow(clippy::cognitive_complexity)]
	pub async fn build(server: Arc<Server>) -> Result<Arc<Self>> {
		let db = Database::open(&server).await?;
		let services = Arc::new(OnceServices { lock: OnceLock::new() });
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

		services
			.lock
			.set(res.clone())
			.map_err(|_| err!("couldn't set services lock"))
			.unwrap();

		Ok(res)
	}

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

	pub async fn stop(&self) {
		info!("Shutting down services...");

		self.interrupt().await;
		if let Some(manager) = self.manager.lock().await.as_ref() {
			manager.stop().await;
		}

		debug_info!("Services shutdown complete.");
	}

	pub async fn poll(&self) -> Result {
		if let Some(manager) = self.manager.lock().await.as_ref() {
			return manager.poll().await;
		}

		Ok(())
	}

	pub(crate) fn services(&self) -> [Arc<dyn Service>; 40] {
		[
			self.account_data.clone(),
			self.admin.clone(),
			self.appservice.clone(),
			self.resolver.clone(),
			self.client.clone(),
			self.config.clone(),
			self.emergency.clone(),
			self.globals.clone(),
			self.key_backups.clone(),
			self.media.clone(),
			self.presence.clone(),
			self.pusher.clone(),
			self.alias.clone(),
			self.auth_chain.clone(),
			self.directory.clone(),
			self.event_handler.clone(),
			self.lazy_loading.clone(),
			self.metadata.clone(),
			self.pdu_metadata.clone(),
			self.read_receipt.clone(),
			self.search.clone(),
			self.short.clone(),
			self.spaces.clone(),
			self.state.clone(),
			self.state_accessor.clone(),
			self.state_cache.clone(),
			self.state_compressor.clone(),
			self.threads.clone(),
			self.timeline.clone(),
			self.typing.clone(),
			self.user.clone(),
			self.federation.clone(),
			self.sending.clone(),
			self.server_keys.clone(),
			self.sync.clone(),
			self.transaction_ids.clone(),
			self.uiaa.clone(),
			self.users.clone(),
			self.membership.clone(),
			self.deactivate.clone(),
		]
	}

	pub async fn clear_cache(&self) {
		futures::stream::iter(self.services())
			.for_each(async |service| {
				service.clear_cache().await;
			})
			.await;
	}

	pub async fn memory_usage(&self) -> Result<String> {
		futures::stream::iter(self.services())
			.map(Ok)
			.try_fold(String::new(), async |mut out, service| {
				service.memory_usage(&mut out).await?;
				Ok(out)
			})
			.await
	}

	async fn interrupt(&self) {
		debug!("Interrupting services...");
		for service in self.services() {
			let name = service.name();
			trace!("Interrupting {name}");
			service.interrupt().await;
		}
	}
}
