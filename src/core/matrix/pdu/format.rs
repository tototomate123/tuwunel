use ruma::{CanonicalJsonObject, CanonicalJsonValue, RoomVersionId};

use crate::{is_equal_to, matrix::room_version};

pub fn into_outgoing_federation(
	mut pdu_json: CanonicalJsonObject,
	room_version: &RoomVersionId,
) -> CanonicalJsonObject {
	if let Some(unsigned) = pdu_json
		.get_mut("unsigned")
		.and_then(|val| val.as_object_mut())
	{
		unsigned.remove("transaction_id");
	}

	let Ok(room_rules) = room_version::rules(room_version) else {
		pdu_json.remove("event_id");
		return pdu_json;
	};

	if !room_rules.event_format.require_event_id {
		pdu_json.remove("event_id");
	}

	if !room_rules
		.event_format
		.require_room_create_room_id
	{
		if pdu_json
			.get("type")
			.and_then(CanonicalJsonValue::as_str)
			.is_some_and(is_equal_to!("m.room.create"))
		{
			pdu_json.remove("room_id");
		}
	}

	pdu_json
}
