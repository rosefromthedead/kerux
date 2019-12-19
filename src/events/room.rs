use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// m.room.create
#[derive(Debug, Deserialize, Serialize)]
pub struct Create {
    pub creator: String,
    pub room_version: Option<String>,
    pub predecessor: Option<PreviousRoom>,
    #[serde(flatten)]
    pub extra: HashMap<String, JsonValue>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PreviousRoom {
    pub room_id: String,
    pub event_id: String,
}

/// m.room.join_rules
#[derive(Debug, Deserialize, Serialize)]
pub struct JoinRules {
    pub join_rule: JoinRule,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinRule {
    Public,
    Knock,
    Invite,
    Private,
}

/// m.room.history_visibility
#[derive(Debug, Deserialize, Serialize)]
pub struct HistoryVisibility {
    pub history_visibility: HistoryVisibilityType,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryVisibilityType {
    Invited,
    Joined,
    Shared,
    WorldReadable,
}

/// m.room.guest_access
#[derive(Debug, Deserialize, Serialize)]
pub struct GuestAccess {
    pub guest_access: GuestAccessType,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestAccessType {
    CanJoin,
    Forbidden,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Name {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Topic {
    pub topic: String,
}

/// m.room.power_levels
#[derive(Debug, Deserialize, Serialize)]
pub struct PowerLevels {
    ban: u32,
    events: HashMap<String, u32>,
    events_default: u32,
    invite: u32,
    kick: u32,
    redact: u32,
    state_default: u32,
    users: HashMap<String, u32>,
    users_default: u32,
    notifications: Notifications,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Notifications {
    room: u32,
}

impl Default for PowerLevels {
    fn default() -> Self {
        PowerLevels {
            ban: 50,
            events: HashMap::new(),
            events_default: 0,
            invite: 50,
            kick: 50,
            redact: 50,
            state_default: 50,
            users: HashMap::new(),
            users_default: 50,
            notifications: Notifications::default(),
        }
    }
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications {
            room: 50,
        }
    }
}

/// m.room.member
#[derive(Debug, Deserialize, Serialize)]
pub struct Member {
    pub avatar_url: Option<String>,
    pub displayname: Option<String>,
    pub membership: Membership,
    pub is_direct: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    Invite,
    Join,
    Knock,
    Leave,
    Ban,
}
