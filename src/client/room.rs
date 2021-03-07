use actix_web::{post, web::{Data, Json, Path}};
use tracing::{Level, Span, instrument};
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
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
        room, Event, EventContent, UnhashedPdu,
    },
    storage::{Storage, StorageManager, UserProfile},
    util::{MatrixId, StorageExt},
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
    content: JsonValue,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Preset {
    PrivateChat,
    TrustedPrivateChat,
    PublicChat,
}

#[post("/createRoom")]
#[instrument(skip_all, err = Level::DEBUG)]
pub async fn create_room(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req: Json<CreateRoomRequest>,
) -> Result<Json<JsonValue>, Error> {
    let req = req.into_inner();
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    Span::current().record("username", &username.as_str());
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    let room_version = req.room_version.unwrap_or("4".to_string());
    let is_direct = req.is_direct.unwrap_or(false);
    if room_version != "4" {
        return Err(Error::UnsupportedRoomVersion);
    }

    let room_id = format!("!{:016X}:{}", rand::random::<i64>(), state.config.domain);
    let mut events = Vec::new();
    let now = chrono::Utc::now().timestamp_millis();

    let room_create = room::Create {
        creator: user_id.clone(),
        room_version: Some(room_version),
        predecessor: None,
        extra: match req.creation_content {
            Some(v) => v,
            None => HashMap::new(),
        },
    };
    let room_create_event = UnhashedPdu {
        event_content: EventContent::Create(room_create),
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        state_key: Some(String::new()),
        prev_events: Vec::new(),
        depth: 0,
        auth_events: Vec::new(),
        redacts: None,
        unsigned: None,
    }.finalize();

    let creator_join = {
        let UserProfile { avatar_url, displayname } = db.get_profile(&username).await?.unwrap();
        room::Member {
            avatar_url,
            displayname,
            membership: room::Membership::Join,
            is_direct,
        }
    };
    let creator_join_event = UnhashedPdu {
        event_content: EventContent::Member(creator_join),
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        state_key: Some(user_id.clone_inner()),
        prev_events: vec![room_create_event.hashes.sha256.clone()],
        depth: 1,
        auth_events: Vec::new(),
        redacts: None,
        unsigned: None,
    }.finalize();

    let power_levels_event = UnhashedPdu {
        event_content: EventContent::PowerLevels(
            req.power_level_content_override.unwrap_or_default()),
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        state_key: Some(String::new()),
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
        event_content: EventContent::JoinRules(room::JoinRules { join_rule }),
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        state_key: Some(String::new()),
        prev_events: vec![power_levels_event_hash.clone()],
        depth: 3,
        auth_events: vec![power_levels_event_hash.clone()],
        redacts: None,
        unsigned: None,
    }.finalize();
    let history_visibility_event = UnhashedPdu {
        event_content: EventContent::HistoryVisibility(room::HistoryVisibility {
            history_visibility
        }),
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        state_key: Some(String::new()),
        prev_events: vec![join_rules_event.hashes.sha256.clone()],
        depth: 4,
        auth_events: vec![power_levels_event_hash.clone()],
        redacts: None,
        unsigned: None,
    }.finalize();
    let guest_access_event = UnhashedPdu {
        event_content: EventContent::GuestAccess(room::GuestAccess { guest_access }),
        room_id: room_id.clone(),
        sender: user_id.clone(),
        origin: state.config.domain.clone(),
        origin_server_ts: now,
        state_key: Some(String::new()),
        prev_events: vec![history_visibility_event.hashes.sha256.clone()],
        depth: 5,
        auth_events: vec![power_levels_event_hash.clone()],
        redacts: None,
        unsigned: None,
    }.finalize();

    events.push(room_create_event);
    events.push(creator_join_event);
    events.push(power_levels_event);
    events.push(join_rules_event);
    events.push(history_visibility_event);
    events.push(guest_access_event);

    for event in req.initial_state.into_iter().flatten() {
        let depth = events.len();
        events.push(
            UnhashedPdu {
                event_content: EventContent::new(&event.ty, event.content)?,
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                state_key: Some(event.state_key),
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
                event_content: EventContent::Name(room::Name { name }),
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                state_key: Some(String::new()),
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
                event_content: EventContent::Topic(room::Topic { topic }),
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                state_key: Some(String::new()),
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
                event_content: EventContent::Member(room::Member {
                    avatar_url: None,
                    displayname: None,
                    membership: room::Membership::Invite,
                    is_direct,
                }),
                room_id: room_id.clone(),
                sender: user_id.clone(),
                origin: state.config.domain.clone(),
                origin_server_ts: now,
                state_key: Some(invitee),
                prev_events: vec![events[depth - 1].hashes.sha256.clone()],
                depth: depth as i64,
                auth_events: vec![power_levels_event_hash.clone()],
                redacts: None,
                unsigned: None,
            }.finalize()
        );
    }

    db.add_pdus(&events).await?;

    tracing::info!(room_id = room_id.as_str(), "Created room");

    Ok(Json(json!({
        "room_id": room_id
    })))
}

#[derive(Deserialize)]
pub struct InviteRequest {
    user_id: MatrixId,
}

#[post("/rooms/{room_id}/invite")]
#[instrument(skip(state, token, req), fields(room_id = &room_id.as_str()), err = Level::DEBUG)]
pub async fn invite(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    room_id: Path<String>,
    req: Json<InviteRequest>,
) -> Result<Json<()>, Error> {
    let mut db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    Span::current().record("username", &username.as_str());
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();
    let invitee = req.into_inner().user_id;
    let invitee_profile = db.get_profile(&invitee.localpart()).await?.unwrap_or_default();

    let invite_event = Event {
        event_content: EventContent::Member(room::Member {
            avatar_url: invitee_profile.avatar_url,
            displayname: invitee_profile.displayname,
            membership: room::Membership::Invite,
            is_direct: false,
        }),
        sender: user_id.clone(),
        room_id: Some(room_id.into_inner()),
        state_key: Some(invitee.clone_inner()),
        unsigned: None,
        redacts: None,
        event_id: None,
        origin_server_ts: None,
    };

    db.add_event(invite_event).await?;

    Ok(Json(()))
}

#[post("/join/{room_id_or_alias}")]
#[instrument(
    skip(state, token), fields(room_id_or_alias = &room_id_or_alias.as_str()), err = Level::DEBUG)]
pub async fn join_by_id_or_alias(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    room_id_or_alias: Path<String>,
) -> Result<Json<JsonValue>, Error> {
    //TODO: implement server_name and third_party_signed args, and room aliases
    let mut db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    Span::current().record("username", &username.as_str());
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();
    let profile = db.get_profile(&username).await?.unwrap_or_default();

    let event = Event {
        event_content: EventContent::Member(room::Member {
            avatar_url: profile.avatar_url,
            displayname: profile.displayname,
            membership: room::Membership::Join,
            is_direct: false,
        }),
        room_id: Some(room_id_or_alias.clone()),   //TODO: what even is an alias
        sender: user_id.clone(),
        state_key: Some(user_id.to_string()),
        unsigned: None,
        redacts: None,
        event_id: None,
        origin_server_ts: None,
    };

    db.add_event(event).await?;

    Ok(Json(serde_json::json!({
        "room_id": *room_id_or_alias
    })))
}
