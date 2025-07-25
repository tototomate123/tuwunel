use std::{convert::AsRef, fmt::Debug};

use tuwunel_core::implement;

use crate::util::or_else;

#[implement(super::Map)]
#[tracing::instrument(skip(self, key), fields(%self), level = "trace")]
pub fn remove<K>(&self, key: &K)
where
	K: AsRef<[u8]> + ?Sized + Debug,
{
	let write_options = &self.write_options;
	self.db
		.db
		.delete_cf_opt(&self.cf(), key, write_options)
		.or_else(or_else)
		.expect("database remove error");

	if !self.db.corked() {
		self.db.flush().expect("database flush error");
	}
}
