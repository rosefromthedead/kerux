use ring::digest::{SHA256, digest};
use serde::{de::Error, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use serde_canonical::ser::to_string as to_canonical_json;
use serde_json::{Map, Value as JsonValue};

use crate::util::MatrixId;

pub mod ephemeral;
pub mod room;

macro_rules! define_event_content {
    (
        $(#[$attr:meta])*
        pub enum EventContent {
            $(
            #[ty = $ty:literal]
            $variant_name:ident($content_type:ty),
            )*
            Unknown {
                ty: String,
                content: JsonValue,
            },
        }
    ) => {
        $(#[$attr])*
        pub enum EventContent {
            $(
            $variant_name($content_type),
            )*
            Unknown {
                ty: String,
                content: JsonValue
            },
        }

        impl EventContent {
            pub fn new(ty: &str, content: JsonValue) -> Result<Self, serde_json::Error> {
                match ty {
                    $(
                    $ty => Ok(EventContent::$variant_name(serde_json::from_value(content)?)),
                    )*
                    _ => Ok(EventContent::Unknown { ty: String::from(ty), content }),
                }
            }

            pub fn get_type(&self) -> &str {
                use EventContent::*;
                match self {
                    $(
                    $variant_name(_) => $ty,
                    )*
                    Unknown { ty, .. } => ty,
                }
            }

            pub fn content_as_json(&self) -> JsonValue {
                use EventContent::*;
                match self {
                    $(
                    $variant_name(v) => serde_json::to_value(&v).unwrap(),
                    )*
                    Unknown { content, .. } => content.clone(),
                }
            }
        }

        impl<'de> Deserialize<'de> for EventContent {
            fn deserialize<D>(d: D) -> Result<Self, D::Error>
            where
                    D: Deserializer<'de> {
                #[derive(Deserialize)]
                struct Intermediate {
                    #[serde(rename = "type")]
                    ty: String,
                    content: JsonValue,
                }
                let value = Intermediate::deserialize(d)?;
                match &*value.ty {
                    $(
                    $ty => serde_json::from_value(value.content)
                        .map(EventContent::$variant_name).map_err(D::Error::custom),
                    )*

                    _ => Ok(EventContent::Unknown {
                        ty: value.ty,
                        content: value.content,
                    }),
                }
            }
        }

        impl Serialize for EventContent {
            fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
                where
            S: Serializer {
                let mut state = s.serialize_struct("", 2)?;
                use EventContent::*;
                match self {
                    $(
                    $variant_name(value) => {
                        state.serialize_field("type", $ty)?;
                        state.serialize_field("content", value)?;
                    },
                    )*

                    Unknown { ty, content } => {
                        state.serialize_field("type", ty)?;
                        state.serialize_field("content", content)?;
                    },
                };
                state.end()
            }
        }
    };
}

define_event_content! {
    #[derive(Clone, Debug)]
    pub enum EventContent {
        #[ty = "m.room.create"]
        Create(room::Create),
        #[ty = "m.room.join_rules"]
        JoinRules(room::JoinRules),
        #[ty = "m.room.history_visibility"]
        HistoryVisibility(room::HistoryVisibility),
        #[ty = "m.room.guest_access"]
        GuestAccess(room::GuestAccess),
        #[ty = "m.room.name"]
        Name(room::Name),
        #[ty = "m.room.topic"]
        Topic(room::Topic),
        #[ty = "m.room.power_levels"]
        PowerLevels(room::PowerLevels),
        #[ty = "m.room.member"]
        Member(room::Member),

        Unknown {
            ty: String,
            content: JsonValue,
        },
    }
}

#[derive(Debug, Serialize)]
pub struct Event {
    #[serde(flatten)]
    pub event_content: EventContent,
    pub sender: MatrixId,
    /// Sometimes this is present outside this struct, in which case None is used
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_key: Option<String>,
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
            event_id: Some(event_id),
            origin_server_ts: Some(self.origin_server_ts),
        }
    }

    pub fn event_id(&self) -> String {
        format!("${}", self.hashes.sha256)
    }
}
