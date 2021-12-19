use serde::{de::Error, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::util::MatrixId;

pub mod ephemeral;
pub mod pdu;
pub mod room;
pub mod room_version;

pub trait EventType: std::convert::TryFrom<EventContent> + Into<EventContent> {
    const EVENT_TYPE: &'static str;
}

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

        $(
        impl EventType for $content_type {
            const EVENT_TYPE: &'static str = $ty;
        }

        impl std::convert::TryFrom<EventContent> for $content_type {
            type Error = EventContent;
            fn try_from(content: EventContent) -> Result<$content_type, EventContent> {
                use EventContent::*;
                match content {
                    $variant_name(v) => Ok(v),
                    _ => Err(content),
                }
            }
        }

        impl From<$content_type> for EventContent {
            fn from(v: $content_type) -> Self {
                EventContent::$variant_name(v)
            }
        }
        )*

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
        #[ty = "m.room.redaction"]
        Redaction(room::Redaction),

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
    pub origin_server_ts: Option<i64>,
}
