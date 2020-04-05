use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use serde_canonical::ser::to_string as to_canonical_json;
use serde_json::{Map, Value as JsonValue};

use crate::util::MatrixId;

pub mod room;

#[derive(Debug, Serialize)]
pub struct Event {
    /// Sometimes this is present outside this struct, in which case None is used
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    pub sender: MatrixId,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_key: Option<String>,
    pub content: JsonValue,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsigned: Option<JsonValue>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacts: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_server_ts: Option<i64>,
}

/// An unhashed (incomplete) Persistent Data Unit for room version 4.
/// This can only be used to construct a complete, hashed PDU.
#[derive(Serialize)]
pub struct UnhashedPdu {
    pub room_id: String,
    pub sender: MatrixId,
    #[serde(rename = "type")]
    pub ty: String,
    pub state_key: Option<String>,
    pub content: JsonValue,
    #[serde(skip)]
    pub unsigned: Option<JsonValue>,
    pub redacts: Option<String>,
    pub origin: String,
    pub origin_server_ts: i64,
    pub prev_events: Vec<String>,
    pub depth: i64,
    pub auth_events: Vec<String>,
}

/// A Persistent Data Unit (room event) for room version 4.
#[derive(Clone, Debug)]
pub struct PduV4 {
    pub room_id: String,
    pub sender: MatrixId,
    pub ty: String,
    pub state_key: Option<String>,
    pub content: JsonValue,
    pub unsigned: Option<JsonValue>,
    pub redacts: Option<String>,
    pub origin: String,
    pub origin_server_ts: i64,
    pub prev_events: Vec<String>,
    pub depth: i64,
    pub auth_events: Vec<String>,
    pub hashes: EventHash,
    pub signatures: Map<String, JsonValue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventHash {
    pub sha256: String,
}

impl UnhashedPdu {
    /// Turns self into a hashed RoomEventV4 by hashing its contents.
    ///
    /// Does not add any signatures.
    pub fn finalize(self) -> PduV4 {
        let json = to_canonical_json(&self).unwrap();
        let sha256 = base64::encode_config(digest(&SHA256, json.as_bytes()).as_ref(), base64::URL_SAFE_NO_PAD);
        PduV4 {
           room_id: self.room_id,
           sender: self.sender,
           ty: self.ty,
           state_key: self.state_key,
           content: self.content,
           unsigned: self.unsigned,
           redacts: self.redacts,
           origin: self.origin,
           origin_server_ts: self.origin_server_ts,
           prev_events: self.prev_events,
           depth: self.depth,
           auth_events: self.auth_events,
           hashes: EventHash { sha256 },
           signatures: Map::new(),
        }
    }
}

impl PduV4 {
    /// Turns a PDU into a format which is suitable for clients.
    pub fn to_client_format(self) -> Event {
        Event {
            room_id: Some(self.room_id),
            ty: self.ty,
            sender: self.sender,
            state_key: self.state_key,
            content: self.content,
            unsigned: self.unsigned,
            redacts: self.redacts,
            event_id: Some(self.hashes.sha256),
            origin_server_ts: Some(self.origin_server_ts),
        }
    }
}
