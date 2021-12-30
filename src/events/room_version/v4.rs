use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use serde_canonical::ser::to_string as to_canonical_json;
use serde_json::{Map, Value as JsonValue};

use crate::{events::{Event, EventContent}, util::MatrixId};

/// An unhashed (incomplete) Persistent Data Unit for room version 4.
/// This can only be used to construct a complete, hashed PDU.
#[derive(Serialize)]
pub struct UnhashedPdu {
    #[serde(flatten)]
    pub event_content: EventContent,
    pub room_id: String,
    pub sender: MatrixId,
    pub state_key: Option<String>,
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
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PduV4 {
//  #[serde(flatten)]
    pub event_content: EventContent,
    pub room_id: String,
    pub sender: MatrixId,
    pub state_key: Option<String>,
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
            event_content: self.event_content,
            room_id: self.room_id,
            sender: self.sender,
            state_key: self.state_key,
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
        let event_id = self.event_id();
        Event {
            event_content: self.event_content,
            room_id: Some(self.room_id),
            sender: self.sender,
            state_key: self.state_key,
            unsigned: self.unsigned,
            redacts: self.redacts,
            origin_server_ts: Some(self.origin_server_ts),
        }
    }

    pub fn event_id(&self) -> String {
        // TODO: badly wrong
        format!("${}", self.hashes.sha256)
    }
}
