use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Deserialize, Serialize)]
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

#[derive(Deserialize, Serialize)]
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
