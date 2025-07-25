use std::{
	collections::{BTreeMap, btree_map::Entry},
	future::Future,
	ops::RangeToInclusive,
	sync::Mutex,
};

use futures::pin_mut;
use serde::Serialize;
use tokio::sync::watch::{Sender, channel};
use tuwunel_core::implement;

use crate::keyval::{KeyBuf, serialize_key};

type Watchers = Mutex<BTreeMap<KeyBuf, Sender<()>>>;

#[derive(Default)]
pub(super) struct Watch {
	watchers: Watchers,
}

#[implement(super::Map)]
pub fn watch_prefix<K>(&self, prefix: K) -> impl Future<Output = ()> + Send + '_
where
	K: Serialize,
{
	let prefix = serialize_key(prefix).expect("failed to serialize watch prefix key");
	self.watch_raw_prefix(&prefix)
}

#[implement(super::Map)]
pub fn watch_raw_prefix<'a, K>(&self, prefix: &'a K) -> impl Future<Output = ()> + Send + use<K>
where
	K: AsRef<[u8]> + ?Sized + 'a,
{
	let rx = match self
		.watch
		.watchers
		.lock()
		.expect("locked")
		.entry(prefix.as_ref().into())
	{
		| Entry::Occupied(node) => node.get().subscribe(),
		| Entry::Vacant(node) => {
			let (tx, rx) = channel(());
			node.insert(tx);
			rx
		},
	};

	async move {
		pin_mut!(rx);
		rx.changed()
			.await
			.expect("watcher sender dropped");
	}
}

#[implement(super::Map)]
pub(crate) fn notify<K>(&self, key: &K)
where
	K: AsRef<[u8]> + Ord + ?Sized,
{
	let range = RangeToInclusive::<KeyBuf> { end: key.as_ref().into() };

	let mut watchers = self.watch.watchers.lock().expect("locked");

	watchers
		.range(range)
		.rev()
		.take_while(|(k, _)| key.as_ref().starts_with(k))
		.filter_map(|(k, tx)| tx.send(()).is_err().then_some(k))
		.cloned()
		.collect::<Vec<_>>()
		.into_iter()
		.for_each(|k| {
			watchers.remove(&k);
		});
}
