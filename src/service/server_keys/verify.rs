use ruma::{
	CanonicalJsonObject, CanonicalJsonValue, OwnedEventId, RoomVersionId, signatures::Verified,
};
use serde_json::value::RawValue as RawJsonValue;
use tuwunel_core::{Err, Result, err, implement, matrix::event::gen_event_id_canonical_json};

#[implement(super::Service)]
pub async fn validate_and_add_event_id(
	&self,
	pdu: &RawJsonValue,
	room_version: &RoomVersionId,
) -> Result<(OwnedEventId, CanonicalJsonObject)> {
	let (event_id, mut value) = gen_event_id_canonical_json(pdu, room_version)?;
	if let Err(e) = self
		.verify_event(&value, Some(room_version))
		.await
	{
		return Err!(BadServerResponse(debug_error!(
			"Event {event_id} failed verification: {e:?}"
		)));
	}

	value.insert("event_id".into(), CanonicalJsonValue::String(event_id.as_str().into()));

	Ok((event_id, value))
}

#[implement(super::Service)]
pub async fn validate_and_add_event_id_no_fetch(
	&self,
	pdu: &RawJsonValue,
	room_version: &RoomVersionId,
) -> Result<(OwnedEventId, CanonicalJsonObject)> {
	let (event_id, mut value) = gen_event_id_canonical_json(pdu, room_version)?;
	let room_version_rules = room_version.rules().ok_or_else(|| {
		err!(Request(UnsupportedRoomVersion(
			"Cannot verify event for unknown room version {room_version:?}."
		)))
	})?;

	if !self
		.required_keys_exist(&value, &room_version_rules)
		.await
	{
		return Err!(BadServerResponse(debug_warn!(
			"Event {event_id} cannot be verified: missing keys."
		)));
	}

	if let Err(e) = self
		.verify_event(&value, Some(room_version))
		.await
	{
		return Err!(BadServerResponse(debug_error!(
			"Event {event_id} failed verification: {e:?}"
		)));
	}

	value.insert("event_id".into(), CanonicalJsonValue::String(event_id.as_str().into()));

	Ok((event_id, value))
}

#[implement(super::Service)]
pub async fn verify_event(
	&self,
	event: &CanonicalJsonObject,
	room_version: Option<&RoomVersionId>,
) -> Result<Verified> {
	let room_version = room_version.unwrap_or(&RoomVersionId::V11);
	let room_version_rules = room_version.rules().ok_or_else(|| {
		err!(Request(UnsupportedRoomVersion(
			"Cannot verify event for unknown room version {room_version:?}."
		)))
	})?;

	let event_keys = self
		.get_event_keys(event, &room_version_rules)
		.await?;

	ruma::signatures::verify_event(&event_keys, event, &room_version_rules).map_err(Into::into)
}

#[implement(super::Service)]
pub async fn verify_json(
	&self,
	event: &CanonicalJsonObject,
	room_version: Option<&RoomVersionId>,
) -> Result {
	let room_version = room_version.unwrap_or(&RoomVersionId::V11);
	let room_version_rules = room_version.rules().ok_or_else(|| {
		err!(Request(UnsupportedRoomVersion(
			"Cannot verify json for unknown room version {room_version:?}."
		)))
	})?;

	let event_keys = self
		.get_event_keys(event, &room_version_rules)
		.await?;

	ruma::signatures::verify_json(&event_keys, event).map_err(Into::into)
}
