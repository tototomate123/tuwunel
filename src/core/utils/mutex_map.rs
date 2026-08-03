//! Per-key asynchronous mutual exclusion with automatic entry cleanup.
//!
//! Each key maps to a Tokio mutex shared by its current contenders. Dropping
//! the final guard removes an idle entry once no contender retains it.

use std::{
	fmt::Debug,
	hash::Hash,
	sync::{Arc, TryLockError::WouldBlock},
};

use tokio::sync::OwnedMutexGuard as Omg;

use crate::{Result, err};

/// Provides independent asynchronous mutexes keyed by owned values.
///
/// Lock acquisition creates entries on demand, and callers contending for the
/// same key serialize. Guard drops opportunistically remove idle entries;
/// cancellation races can leave an entry retained.
#[derive(Debug)]
pub struct MutexMap<Key, Val> {
	map: Map<Key, Val>,
}

/// Keeps a keyed mutex locked until the guard is dropped.
///
/// The guard retains the parent map so cleanup remains possible. Dropping it
/// attempts to remove the key when no other holder or contender references the
/// mutex, but cancellation races can leave an idle entry retained.
#[derive(Debug)]
#[clippy::has_significant_drop]
pub struct Guard<Key, Val> {
	map: Map<Key, Val>,
	val: Omg<Val>,
}

type Map<Key, Val> = Arc<MapMutex<Key, Val>>;
type MapMutex<Key, Val> = std::sync::Mutex<HashMap<Key, Val>>;
type HashMap<Key, Val> = std::collections::HashMap<Key, Value<Val>>;
type Value<Val> = Arc<tokio::sync::Mutex<Val>>;

impl<Key, Val> MutexMap<Key, Val>
where
	Key: Clone + Eq + Hash + Send,
	Val: Default + Send,
{
	/// Creates an empty keyed mutex map.
	///
	/// No per-key mutex is allocated until a lock method first sees its key.
	/// The result is equivalent to [`Default::default`].
	#[must_use]
	pub fn new() -> Self {
		Self {
			map: Map::new(MapMutex::new(HashMap::new())),
		}
	}

	/// Acquires the asynchronous mutex associated with a key.
	///
	/// The method creates an entry if absent and waits for the current holder
	/// to release it. The returned guard attempts to clean up the idle entry
	/// when dropped, and a poisoned internal map mutex causes a panic.
	#[tracing::instrument(level = "trace", skip(self))]
	pub async fn lock<K>(&self, k: &K) -> Guard<Key, Val>
	where
		K: Debug + Send + ?Sized + Sync + ToOwned<Owned = Key>,
	{
		let val = self
			.map
			.lock()
			.expect("locked")
			.entry(k.to_owned())
			.or_default()
			.clone();

		Guard::<Key, Val> {
			map: Arc::clone(&self.map),
			val: val.lock_owned().await,
		}
	}

	/// Attempts to acquire a key without waiting for its asynchronous mutex.
	///
	/// The key entry is created if absent, and contention returns an error
	/// instead of yielding. Acquiring the internal map mutex can still block
	/// and panics if that mutex is poisoned.
	#[tracing::instrument(level = "trace", skip(self))]
	pub fn try_lock<K>(&self, k: &K) -> Result<Guard<Key, Val>>
	where
		K: Debug + Send + ?Sized + Sync + ToOwned<Owned = Key>,
	{
		let val = self
			.map
			.lock()
			.expect("locked")
			.entry(k.to_owned())
			.or_default()
			.clone();

		Ok(Guard::<Key, Val> {
			map: Arc::clone(&self.map),
			val: val
				.try_lock_owned()
				.map_err(|_| err!("would yield"))?,
		})
	}

	/// Attempts to acquire a key without blocking or yielding.
	///
	/// Contention on either the internal map or keyed mutex returns an error.
	/// The entry is created only after the map mutex is acquired, and a
	/// poisoned map mutex causes a panic.
	#[tracing::instrument(level = "trace", skip(self))]
	pub fn try_try_lock<K>(&self, k: &K) -> Result<Guard<Key, Val>>
	where
		K: Debug + Send + ?Sized + Sync + ToOwned<Owned = Key>,
	{
		let val = self
			.map
			.try_lock()
			.map_err(|e| match e {
				| WouldBlock => err!("would block"),
				| _ => panic!("{e:?}"),
			})?
			.entry(k.to_owned())
			.or_default()
			.clone();

		Ok(Guard::<Key, Val> {
			map: Arc::clone(&self.map),
			val: val
				.try_lock_owned()
				.map_err(|_| err!("would yield"))?,
		})
	}

	/// Reports whether the map currently contains an entry for a key.
	///
	/// An entry can represent a held mutex, contenders that still reference it,
	/// or an idle mutex retained after a cancellation race. The check locks the
	/// internal map and panics if that mutex is poisoned.
	#[must_use]
	pub fn contains(&self, k: &Key) -> bool { self.map.lock().expect("locked").contains_key(k) }

	/// Reports whether no keyed mutex entries are currently tracked.
	///
	/// A retained idle entry can remain after a waiter is canceled, so a false
	/// result does not imply an active holder or waiter. The check locks the
	/// internal map and panics if that mutex is poisoned.
	#[must_use]
	pub fn is_empty(&self) -> bool { self.map.lock().expect("locked").is_empty() }

	/// Returns the number of keyed mutex entries currently tracked.
	///
	/// The count includes held mutexes, entries retained by contenders, and
	/// idle entries left by cancellation races. The check locks the internal
	/// map and panics if that mutex is poisoned.
	#[must_use]
	pub fn len(&self) -> usize { self.map.lock().expect("locked").len() }
}

impl<Key, Val> Default for MutexMap<Key, Val>
where
	Key: Clone + Eq + Hash + Send,
	Val: Default + Send,
{
	fn default() -> Self { Self::new() }
}

impl<Key, Val> Drop for Guard<Key, Val> {
	#[tracing::instrument(name = "unlock", level = "trace", skip_all)]
	fn drop(&mut self) {
		if Arc::strong_count(Omg::mutex(&self.val)) <= 2 {
			self.map.lock().expect("locked").retain(|_, val| {
				!Arc::ptr_eq(val, Omg::mutex(&self.val)) || Arc::strong_count(val) > 2
			});
		}
	}
}
