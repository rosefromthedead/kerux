use actix_web::{post, web::{Data, Json}};
use serde::Deserialize;
use serde_json::{Map, Value as JsonValue, json};
use std::{
    collections::HashMap,
    sync::Arc,
};

use crate::{
    client::{
        auth::AccessToken,
        error::Error,
    },
    events::{
        room, UnhashedPdu, into_json_map
    },
    ServerState,
};

#[derive(Deserialize)]
pub struct CreateRoomRequest {
    visibility: RoomVisibility,
    room_alias_name: Option<String>,
    name: Option<String>,
    topic: Option<String>,
    invite: Option<Vec<String>>,
    invite_3pid: Option<Vec<Invite3pid>>,
    room_version: Option<String>,
    creation_content: Option<HashMap<String, JsonValue>>,
    initial_state: Option<Vec<StateEvent>>,
    preset: Option<Preset>,
    is_direct: Option<bool>,
    power_level_content_override: Option<crate::events::room::PowerLevels>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RoomVisibility {
    Public,
    Private,
}

#[derive(Deserialize)]
struct Invite3pid {
    id_server: String,
    medium: String,
    address: String,
}

#[derive(Deserialize)]
struct StateEvent {
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    state_key: String,
    content: Map<String, JsonValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Preset {
    PrivateChat,
    TrustedPrivateChat,
    PublicChat,
}

#[post("/createRoom")]
pub async fn create_room(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req: Json<CreateRoomRequest>,
) -> Result<Json<JsonValue>, Error> {
    let req = req.into_inner();
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;
    let user_id = format!("@{}:{}", username, state.config.domain);

    let room_version = req.room_version.unwrap_or("4".to_string());
    let is_direct = req.is_direct.unwrap_or(false);
    if room_version != "4" {
        return Err(Error::UnsupportedRoomVersion);
    }

    let room_id = format!("!{:016X}:{}", rand::random::<i64>(), state.config.domain);
    let mut events = Vec::new();
    let now = chrono::Utc::now().timestamp_millis();

    let room_create = {
        let extra = match req.creation_content {
            Some(v) => v,
            None => HashMap::new(),
        };
        into_json_map(&room::Create {
            creator: user_id.clone(),
            room_version: Some(room_version),
            predecessor: None,
            extra,
        })
    };
    let room_create_event = UnhashedPdu {
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        ty: "m.room.create".to_string(),
        state_key: Some(String::new()),
        content: room_create,
        prev_events: Vec::new(),
        depth: 0,
        auth_events: Vec::new(),
        redacts: None,
        unsigned: None,
    }.finalize();
    
    let creator_join = {
        let (avatar_url, displayname) = db.get_profile(&user_id).await?;
        into_json_map(&room::Member {
            avatar_url,
            displayname,
            membership: room::Membership::Join,
            is_direct,
        })
    };
    let creator_join_event = UnhashedPdu {
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        ty: "m.room.member".to_string(),
        state_key: Some(user_id.clone()),
        content: creator_join,
        prev_events: vec![room_create_event.hashes.sha256.clone()],
        depth: 1,
        auth_events: Vec::new(),
        redacts: None,
        unsigned: None,
    }.finalize();

    let power_levels_event = UnhashedPdu {
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        ty: "m.room.power_levels".to_string(),
        state_key: Some(String::new()),
        content: into_json_map(&req.power_level_content_override.unwrap_or_default()),
        prev_events: vec![creator_join_event.hashes.sha256.clone()],
        depth: 2,
        auth_events: Vec::new(),
        redacts: None,
        unsigned: None,
    }.finalize();
    let power_levels_event_hash = power_levels_event.hashes.sha256.clone();

    let (join_rule, history_visibility, guest_access) = {
        use room::{JoinRule::*, HistoryVisibilityType::*, GuestAccessType::*};
        let preset = req.preset.unwrap_or(match req.visibility {
            RoomVisibility::Private => Preset::PrivateChat,
            RoomVisibility::Public => Preset::PublicChat,
        });
        match preset {
            Preset::PrivateChat | Preset::TrustedPrivateChat => (Invite, Shared, CanJoin),
            Preset::PublicChat => (Public, Shared, Forbidden),
        }
    };
    let join_rules_event = UnhashedPdu {
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        ty: "m.room.join_rules".to_string(),
        state_key: Some(String::new()),
        content: into_json_map(&room::JoinRules { join_rule }),
        prev_events: vec![power_levels_event_hash.clone()],
        depth: 3,
        auth_events: vec![power_levels_event_hash.clone()],
        redacts: None,
        unsigned: None,
    }.finalize();
    let history_visibility_event = UnhashedPdu {
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        ty: "m.room.history_visibility".to_string(),
        state_key: Some(String::new()),
        content: into_json_map(&room::HistoryVisibility { history_visibility }),
        prev_events: vec![join_rules_event.hashes.sha256.clone()],
        depth: 4,
        auth_events: vec![power_levels_event_hash.clone()],
        redacts: None,
        unsigned: None,
    }.finalize();
    let guest_access_event = UnhashedPdu {
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        ty: "m.room.guest_access".to_string(),
        state_key: Some(String::new()),
        content: into_json_map(&room::GuestAccess { guest_access }),
        prev_events: vec![history_visibility_event.hashes.sha256.clone()],
        depth: 5,
        auth_events: vec![power_levels_event_hash.clone()],
        redacts: None,
        unsigned: None,
    }.finalize();

    events.push(room_create_event);
    events.push(power_levels_event);
    events.push(join_rules_event);
    events.push(history_visibility_event);
    events.push(guest_access_event);

    for event in req.initial_state.into_iter().flatten() {
        let depth = events.len();
        events.push(
            UnhashedPdu {
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                ty: event.ty,
                state_key: Some(event.state_key),
                content: event.content,
                prev_events: vec![events[depth - 1].hashes.sha256.clone()],
                depth: depth as i64,
                auth_events: vec![power_levels_event_hash.clone()],
                redacts: None,
                unsigned: None,
            }.finalize()
        );
    }

    if let Some(name) = req.name {
        let depth = events.len();
        events.push(
            UnhashedPdu {
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                ty: "m.room.name".to_string(),
                state_key: Some(String::new()),
                content: into_json_map(&room::Name { name }),
                prev_events: vec![events[depth - 1].hashes.sha256.clone()],
                depth: depth as i64,
                auth_events: vec![power_levels_event_hash.clone()],
                redacts: None,
                unsigned: None,
            }.finalize()
        );
    }

    if let Some(topic) = req.topic {
        let depth = events.len();
        events.push(
            UnhashedPdu {
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                ty: "m.room.topic".to_string(),
                state_key: Some(String::new()),
                content: into_json_map(&room::Topic { topic }),
                prev_events: vec![events[depth - 1].hashes.sha256.clone()],
                depth: depth as i64,
                auth_events: vec![power_levels_event_hash.clone()],
                redacts: None,
                unsigned: None,
            }.finalize()
        );
    }

    for invitee in req.invite.into_iter().flatten() {
        let depth = events.len();
        events.push(
            UnhashedPdu {
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                ty: "m.room.member".to_string(),
                state_key: Some(invitee),
                content: into_json_map(&room::Member {
                    avatar_url: None,
                    displayname: None,
                    membership: room::Membership::Invite,
                    is_direct,
                }),
                prev_events: vec![events[depth - 1].hashes.sha256.clone()],
                depth: depth as i64,
                auth_events: vec![power_levels_event_hash.clone()],
                redacts: None,
                unsigned: None,
            }.finalize()
        );
    }

    db.add_events(events).await?;

    Ok(Json(json!({
        "room_id": room_id
    })))
}
