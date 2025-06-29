use ruma::{CanonicalJsonObject, RoomVersionId};
use tuwunel_core::{Result, err, implement};

#[implement(super::Service)]
pub fn sign_json(&self, object: &mut CanonicalJsonObject) -> Result {
	use ruma::signatures::sign_json;

	let server_name = self.services.globals.server_name().as_str();
	sign_json(server_name, self.keypair(), object).map_err(Into::into)
}

#[implement(super::Service)]
pub fn hash_and_sign_event(
	&self,
	object: &mut CanonicalJsonObject,
	room_version: &RoomVersionId,
) -> Result {
	use ruma::signatures::hash_and_sign_event;

	let server_name = &self.services.server.name;
	let room_version_rules = room_version.rules().ok_or_else(|| {
		err!(Request(UnsupportedRoomVersion(
			"Cannot hash and sign event for unknown room version {room_version:?}."
		)))
	})?;

	hash_and_sign_event(
		server_name.as_str(),
		self.keypair(),
		object,
		&room_version_rules.redaction,
	)
	.map_err(Into::into)
}
