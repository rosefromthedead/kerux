use actix_web::{get, put, web::{Data, Json, Path, Query}};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::{
    collections::HashMap,
    sync::Arc
};

use crate::{
    client::{
        auth::AccessToken,
        error::Error,
    },
    events::{
        Event, UnhashedPdu,
        room::Membership,
    },
    ServerState,
};

/// Provided in URL query params
#[derive(Debug, Deserialize)]
pub struct SyncRequest {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    full_state: bool,
    #[serde(default = "default_set_presence")]
    set_presence: SetPresence,
    #[serde(default)]
    timeout: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename = "snake_case")]
enum SetPresence {
    Offline,
    Online,
    Unavailable,
}

fn default_set_presence() -> SetPresence {
    SetPresence::Online
}

#[derive(Debug, Serialize)]
pub struct SyncResponse {
    next_batch: String,
    rooms: Option<Rooms>,
    presence: Option<Presence>,
    account_data: Option<AccountData>,
}

#[derive(Debug, Serialize)]
struct Rooms {
    join: HashMap<String, JoinedRoom>,
    invite: HashMap<String, InvitedRoom>,
    leave: HashMap<String, LeftRoom>,
}

#[derive(Debug, Serialize)]
struct JoinedRoom {
    summary: RoomSummary,
    state: State,
    timeline: Timeline,
    ephemeral: Ephemeral,
    account_data: AccountData,
}

#[derive(Debug, Serialize)]
struct RoomSummary {
    #[serde(rename = "m.heroes")]
    heroes: Option<Vec<String>>,
    #[serde(rename = "m.joined_member_count")]
    joined_member_count: i64,
    #[serde(rename = "m.invited_member_count")]
    invited_member_count: i64,
}

#[derive(Debug, Serialize)]
struct State {
    events: Vec<Event>,
}

#[derive(Debug, Serialize)]
struct Timeline {
    events: Vec<Event>,
    limited: bool,
    prev_batch: String,
}

#[derive(Debug, Serialize)]
struct Ephemeral {
    events: Vec<KvPair>,
}

/// This is referred to as `Event` in the Matrix spec, but we already have a thing called event
/// and it doesn't really make sense to call it that.
#[derive(Debug, Serialize)]
struct KvPair {
    content: JsonValue,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Debug, Serialize)]
struct AccountData {
    events: Vec<KvPair>,
}

#[derive(Debug, Serialize)]
struct InvitedRoom {
    invite_state: InviteState,
}

#[derive(Debug, Serialize)]
struct InviteState {
    events: Vec<StrippedState>,
}

#[derive(Debug, Serialize)]
struct StrippedState {
    content: JsonValue,
    state_key: String,
    #[serde(rename = "type")]
    ty: String,
    sender: String,
}

#[derive(Debug, Serialize)]
struct LeftRoom {
    state: State,
    timeline: Timeline,
    account_data: AccountData,
}

#[derive(Debug, Serialize)]
struct Presence {
    events: Vec<KvPair>,
}

#[get("/sync")]
pub async fn sync(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req: Query<SyncRequest>,
) -> Result<Json<SyncResponse>, Error> {
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;
    let user_id = format!("@{}:{}", username, state.config.domain);

    let memberships = db.get_memberships_by_user(&user_id).await?;
    let mut join = HashMap::new();
    let mut invite = HashMap::new();
    let mut leave = HashMap::new();
    for (room_id, membership) in memberships {
        match membership {
            Membership::Join => {
                let (joined_member_count, invited_member_count)
                    = db.get_room_member_counts(&room_id).await?;
                let summary = RoomSummary {
                    heroes: None,   // TODO
                    joined_member_count,
                    invited_member_count,
                };
                let state = if req.full_state {
                    State { events: db.get_full_state(&room_id).await? }
                } else {
                    State { events: Vec::new() }
                };
                let timeline = Timeline {
                    events: db.get_events_since(&room_id, req.since.as_ref().map(|s| &**s)).await?,
                    limited: false,
                    prev_batch: String::from("placeholder_prev_batch"),
                };
                //TODO: implement ephemeral events and per-room account data
                let ephemeral = Ephemeral {
                    events: Vec::new(),
                };
                let account_data = AccountData {
                    events: Vec::new(),
                };
                join.insert(room_id, JoinedRoom {
                    summary,
                    state,
                    timeline,
                    ephemeral,
                    account_data,
                });
            },
            Membership::Invite => {},
            Membership::Leave => {},
            Membership::Knock => {},
            Membership::Ban => {},
        }
    }
    let rooms = Rooms { join, invite, leave };

    Ok(Json(SyncResponse {
        next_batch: String::new(),
        rooms: Some(rooms),
        presence: None,
        account_data: None,
    }))
}

#[get("/rooms/{room_id}/event/{event_id}")]
pub async fn get_event(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    path_args: Path<(String, String)>,
) -> Result<Json<Event>, Error> {
    let (room_id, event_id) = path_args.into_inner();
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;

    if db.get_membership(
        &format!("@{}:{}", username, state.config.domain),
        &room_id
    ).await? != Some(Membership::Join) {
        return Err(Error::Forbidden);
    }

    match db.get_event(&room_id, &event_id).await? {
        Some(event) => Ok(Json(event)),
        None => Err(Error::NotFound),
    }
}

#[get("/rooms/{room_id}/state/{event_id}")]
pub async fn get_state_event_no_key(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    path_args: Path<(String, String)>,
) -> Result<Json<Event>, Error> {
    let (room_id, event_type) = path_args.into_inner();
    get_state_event_inner(state, token, (room_id, event_type, String::new())).await
}

#[get("/rooms/{room_id}/state/{event_id}/{state_key}")]
pub async fn get_state_event_key(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    path_args: Path<(String, String, String)>,
) -> Result<Json<Event>, Error> {
    get_state_event_inner(state, token, path_args.into_inner()).await
}

pub async fn get_state_event_inner(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    (room_id, event_type, state_key): (String, String, String),
) -> Result<Json<Event>, Error> {
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;

    if db.get_membership(
        &format!("@{}:{}", username, state.config.domain),
        &room_id
    ).await? != Some(Membership::Join) {
        return Err(Error::Forbidden);
    }

    match db.get_state_event(&room_id, &event_type, &state_key).await? {
        Some(event) => Ok(Json(event)),
        None => Err(Error::NotFound),
    }
}

#[get("/rooms/{room_id}/state")]
pub async fn get_state(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    room_id: Path<String>,
) -> Result<Json<Vec<Event>>, Error> {
    let room_id = room_id.into_inner();
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;

    match db.get_membership(
        &format!("@{}:{}", username, state.config.domain),
        &room_id
    ).await? {
        Some(Membership::Join) => {},
        Some(_) => return Err(Error::Unimplemented),
        None => return Err(Error::Forbidden),
    }

    let state = db.get_full_state(&room_id).await?;
    Ok(Json(state))
}

#[derive(Deserialize)]
pub struct MembersRequest {
    at: String,
    #[serde(default)]
    membership: Option<Membership>,
    #[serde(default)]
    not_membership: Option<Membership>,
}

#[derive(Serialize)]
pub struct MembersResponse {
    chunk: Vec<Event>,
}

#[get("/rooms/{room_id}/members")]
pub async fn get_members(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    room_id: Path<String>,
    req: Query<MembersRequest>,
) -> Result<Json<MembersResponse>, Error> {
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;
    
    match db.get_membership(
        &format!("@{}:{}", username, state.config.domain),
        &room_id
    ).await? {
        Some(Membership::Join) => {},
        Some(_) => return Err(Error::Unimplemented),
        None => return Err(Error::Forbidden),
    }

    let mut state = db.get_full_state(&room_id).await?;
    state.retain(|event| {
        let membership: Membership = event.content.get("membership")
            .expect("no membership in m.room.member")
            .as_str()
            .expect("membership is not a string")
            .parse()
            .unwrap();
        event.ty == "m.room.member"
        && if let Some(filter) = &req.membership { membership == *filter } else { true }
        && if let Some(exclude) = &req.not_membership { membership != *exclude } else { true }
    });

    Ok(Json(MembersResponse { chunk: state }))
}

#[derive(Serialize)]
pub struct SendEventResponse {
    event_id: String,
}

#[put("/rooms/{room_id}/state/{event_type}/{state_key}")]
pub async fn send_state_event(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req: Path<(String, String, String)>,
    event_content: Json<JsonValue>,
) -> Result<Json<SendEventResponse>, Error> {
    let (room_id, event_type, state_key) = req.into_inner();
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;
    let user_id = format!("@{}:{}", username, state.config.domain);

    if db.get_membership(
        &format!("@{}:{}", username, state.config.domain),
        &room_id
    ).await? != Some(Membership::Join) {
        return Err(Error::Forbidden);
    }

    let (depth, prev_events) = match db.get_prev_event_ids(&room_id).await? {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };
    let event = UnhashedPdu {
        room_id,
        sender: user_id,
        origin: state.config.domain.clone(),
        origin_server_ts: chrono::Utc::now().timestamp_millis(),
        ty: event_type,
        state_key: Some(state_key),
        content: event_content.into_inner(),
        prev_events,
        depth,
        auth_events: Vec::new(),    //TODO: permissions and whatnot
        redacts: None,
        unsigned: None,
    }.finalize();

    let mut event_id = event.hashes.sha256.clone();
    event_id.insert(0, '$');

    db.add_events(&[event]).await?;

    Ok(Json(SendEventResponse {
        event_id,
    }))
}

#[put("/rooms/{room_id}/send/{event_type}/{txn_id}")]
pub async fn send_event(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req: Path<(String, String, String)>,
    event_content: Json<JsonValue>,
) -> Result<Json<SendEventResponse>, Error> {
    let (room_id, event_type, state_key) = req.into_inner();
    let mut db = state.db_pool.get_client().await?;
    let username = db.try_auth(token.0).await?;
    let user_id = format!("@{}:{}", username, state.config.domain);

    if db.get_membership(
        &format!("@{}:{}", username, state.config.domain),
        &room_id
    ).await? != Some(Membership::Join) {
        return Err(Error::Forbidden);
    }

    let (depth, prev_events) = match db.get_prev_event_ids(&room_id).await? {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };
    let event = UnhashedPdu {
        room_id,
        sender: user_id,
        origin: state.config.domain.clone(),
        origin_server_ts: chrono::Utc::now().timestamp_millis(),
        ty: event_type,
        state_key: None,
        content: event_content.into_inner(),
        prev_events,
        depth,
        auth_events: Vec::new(),    //TODO: permissions and whatnot
        redacts: None,
        unsigned: None,
    }.finalize();

    let mut event_id = event.hashes.sha256.clone();
    event_id.insert(0, '$');

    db.add_events(&[event]).await?;

    Ok(Json(SendEventResponse {
        event_id,
    }))
}
