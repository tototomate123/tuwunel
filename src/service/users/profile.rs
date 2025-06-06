use futures::{Stream, StreamExt, TryFutureExt};
use ruma::UserId;
use tuwunel_core::{Result, implement, utils::stream::TryIgnore};
use tuwunel_database::{Deserialized, Ignore, Interfix, Json};

/// Gets a specific user profile key
#[implement(super::Service)]
pub async fn profile_key(
	&self,
	user_id: &UserId,
	profile_key: &str,
) -> Result<serde_json::Value> {
	let key = (user_id, profile_key);
	self.db
		.useridprofilekey_value
		.qry(&key)
		.await
		.deserialized()
}

/// Gets all the user's profile keys and values in an iterator
#[implement(super::Service)]
pub fn all_profile_keys<'a>(
	&'a self,
	user_id: &'a UserId,
) -> impl Stream<Item = (String, serde_json::Value)> + 'a + Send {
	type KeyVal = ((Ignore, String), serde_json::Value);

	let prefix = (user_id, Interfix);
	self.db
		.useridprofilekey_value
		.stream_prefix(&prefix)
		.ignore_err()
		.map(|((_, key), val): KeyVal| (key, val))
}

/// Sets a new profile key value, removes the key if value is None
#[implement(super::Service)]
pub fn set_profile_key(
	&self,
	user_id: &UserId,
	profile_key: &str,
	profile_key_value: Option<serde_json::Value>,
) {
	// TODO: insert to the stable MSC4175 key when it's stable
	let key = (user_id, profile_key);

	if let Some(value) = profile_key_value {
		self.db
			.useridprofilekey_value
			.put(key, Json(value));
	} else {
		self.db.useridprofilekey_value.del(key);
	}
}

/// Get the timezone of a user.
#[implement(super::Service)]
pub async fn timezone(&self, user_id: &UserId) -> Result<String> {
	// TODO: transparently migrate unstable key usage to the stable key once MSC4133
	// and MSC4175 are stable, likely a remove/insert in this block.

	// first check the unstable prefix then check the stable prefix
	let unstable_key = (user_id, "us.cloke.msc4175.tz");
	let stable_key = (user_id, "m.tz");
	self.db
		.useridprofilekey_value
		.qry(&unstable_key)
		.or_else(|_| self.db.useridprofilekey_value.qry(&stable_key))
		.await
		.deserialized()
}

/// Sets a new timezone or removes it if timezone is None.
#[implement(super::Service)]
pub fn set_timezone(&self, user_id: &UserId, timezone: Option<String>) {
	// TODO: insert to the stable MSC4175 key when it's stable
	let key = (user_id, "us.cloke.msc4175.tz");

	if let Some(timezone) = timezone {
		self.db
			.useridprofilekey_value
			.put_raw(key, &timezone);
	} else {
		self.db.useridprofilekey_value.del(key);
	}
}
