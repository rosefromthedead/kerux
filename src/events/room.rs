use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use crate::util::MatrixId;

/// m.room.create
#[derive(Debug, Deserialize, Serialize)]
pub struct Create {
    pub creator: MatrixId,
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

#[derive(Debug, PartialEq, Deserialize, Serialize)]
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
    ban: Option<u32>,
    invite: Option<u32>,
    kick: Option<u32>,
    redact: Option<u32>,
    events: HashMap<String, u32>,
    events_default: Option<u32>,
    state_default: Option<u32>,
    users: HashMap<MatrixId, u32>,
    users_default: Option<u32>,
    notifications: Notifications,
}

impl PowerLevels {
    /// This function returns the effective power levels for when a room has no power levels event.
    /// The values are the same as when there is an event but the values are unspecified
    /// (i.e. `None`), with the exception that state_default is 0 and the creator of the room has
    /// power level 100.
    pub fn no_event_default_levels(room_creator: &MatrixId) -> Self {
        let mut users = HashMap::new();
        users.insert(room_creator.clone(), 100);
        PowerLevels {
            ban: Some(50),
            invite: Some(50),
            kick: Some(50),
            redact: Some(50),
            events: HashMap::new(),
            events_default: Some(0),
            state_default: Some(0),
            users,
            users_default: Some(0),
            notifications: Notifications::default(),
        }
    }

    pub fn ban(&self) -> u32 {
        self.ban.unwrap_or(50)
    }

    pub fn invite(&self) -> u32 {
        self.invite.unwrap_or(50)
    }

    pub fn kick(&self) -> u32 {
        self.kick.unwrap_or(50)
    }

    pub fn redact(&self) -> u32 {
        self.kick.unwrap_or(50)
    }

    pub fn get_user_level(&self, user_id: &MatrixId) -> u32 {
        self.users.get(user_id).copied().unwrap_or(self.users_default.unwrap_or(0))
    }

    pub fn get_event_level(&self, event_type: &str, is_state_event: bool) -> u32 {
        let default = if is_state_event {
            self.state_default.unwrap_or(50)
        } else {
            self.events_default.unwrap_or(0)
        };
        self.events.get(event_type).copied().unwrap_or(default)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Notifications {
    room: u32,
}

impl Default for PowerLevels {
    fn default() -> Self {
        PowerLevels {
            ban: Some(50),
            events: HashMap::new(),
            events_default: Some(0),
            invite: Some(50),
            kick: Some(50),
            redact: Some(50),
            state_default: Some(50),
            users: HashMap::new(),
            users_default: Some(50),
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

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    Invite,
    Join,
    Knock,
    Leave,
    Ban,
}

#[derive(Debug)]
pub struct InvalidMembership(String);

impl std::str::FromStr for Membership {
    type Err = InvalidMembership;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ban" => Ok(Membership::Ban),
            "invite" => Ok(Membership::Invite),
            "join" => Ok(Membership::Join),
            "knock" => Ok(Membership::Knock),
            "leave" => Ok(Membership::Leave),
            x => Err(InvalidMembership(x.to_string())),
        }
    }
}

impl ToString for Membership {
    fn to_string(&self) -> String {
        use Membership::*;
        match self {
            Ban => "ban",
            Invite => "invite",
            Join => "join",
            Knock => "knock",
            Leave => "leave",
        }.to_string()
    }
}
