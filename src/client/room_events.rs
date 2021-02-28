use actix_web::{get, put, web::{Data, Json, Path, Query}};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::{
    collections::HashMap,
    sync::Arc
};
use tokio::time::{Duration, delay_for};

use crate::{
    client::{
        auth::AccessToken,
        error::Error,
    },
    events::{
        Event, EventContent,
        room::Membership,
    },
    storage::{EventQuery, Storage, StorageManager, QueryType},
    util::{MatrixId, StorageExt},
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
    account_data: AccountData,
}

#[derive(Debug, Default, Serialize)]
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
    joined_member_count: usize,
    #[serde(rename = "m.invited_member_count")]
    invited_member_count: usize,
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
    sender: MatrixId,
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
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    let mut batch = db.get_batch(req.since.as_deref().unwrap_or("empty")).await?.unwrap_or_default();
    let next_batch_id = format!("{:x}", rand::random::<u64>());
    let mut res = SyncResponse {
        next_batch: next_batch_id.clone(),
        rooms: None,
        presence: None,
        account_data: AccountData {
            events: Vec::new(),
        },
    };

    let memberships = db.get_memberships_by_user(&user_id).await?;
    let mut something_happened = false;
    for (room_id, membership) in memberships.iter() {
        match membership {
            Membership::Join => {
                batch.invites.remove(room_id);
                let from = batch.rooms.get(room_id).map(|v| *v).unwrap_or(0);
                let (events, progress) = db.query_events(EventQuery {
                    query_type: QueryType::Timeline { from, to: None },
                    room_id,
                    senders: &[],
                    not_senders: &[],
                    types: &[],
                    not_types: &[],
                    contains_json: None,
                }, false).await?;
                batch.rooms.insert(room_id.clone(), progress + 1);

                let mut state_events = Vec::new();
                if req.full_state {
                    state_events = db.get_full_state(&room_id).await?;
                }

                if !events.is_empty() || !state_events.is_empty() {
                    something_happened = true;
                    let (joined, invited) = db.get_room_member_counts(&room_id).await?;
                    let summary = RoomSummary {
                        heroes: None,
                        joined_member_count: joined,
                        invited_member_count: invited,
                    };
                    let state = State { events: state_events };
                    let timeline = Timeline {
                        events,
                        limited: false,
                        prev_batch: String::from("empty"),
                    };
                    let ephemeral = Ephemeral {
                        events: db.get_all_ephemeral(room_id).await?.into_iter().map(
                            |(k, v)| KvPair {
                                ty: k,
                                content: v,
                            }).collect()
                    };
                    let account_data = AccountData { events: Vec::new() };
                    res.rooms.get_or_insert_with(Default::default).join.insert(
                        String::from(room_id),
                        JoinedRoom {
                            summary,
                            state,
                            timeline,
                            ephemeral,
                            account_data,
                        },
                    );
                }
            },
            Membership::Invite if !batch.invites.contains(room_id) => {
                let events = db.get_full_state(&room_id).await?
                    .into_iter()
                    .map(|e| StrippedState {
                        content: e.content,
                        state_key: e.state_key.unwrap(),
                        ty: e.ty,
                        sender: e.sender,
                    })
                    .collect();
                res.rooms.get_or_insert_with(Default::default).invite.insert(
                    room_id.clone(),
                    InvitedRoom {
                        invite_state: InviteState {
                            events,
                        },
                    },
                );
                batch.invites.insert(room_id.clone());
            }
            _ => {},
        }
    }

    if something_happened {
        db.set_batch(&next_batch_id, batch).await?;
        return Ok(Json(res));
    }

    let mut queries = Vec::new();
    for (room_id, _) in memberships.iter().filter(|(_, m)| **m == Membership::Join) {
        let from = batch.rooms.get(room_id).map(|v| *v).unwrap_or(0);
        let room_id_clone = String::from(room_id);
        queries.push(db.query_events(EventQuery {
            query_type: QueryType::Timeline {
                from, to: None,
            },
            room_id,
            senders: &[],
            not_senders: &[],
            types: &[],
            not_types: &[],
            contains_json: None,
        }, true).map(move |r| (r, room_id_clone)));
    }
    if queries.is_empty() {
        // user is not in any rooms. no point waiting for stuff to happen in them
        db.set_batch(&next_batch_id, batch).await?;
        return Ok(Json(res));
    }

    let timeout = delay_for(Duration::from_millis(req.timeout as _));
    tokio::select! {
        _ = timeout => {
            db.set_batch(&next_batch_id, batch).await?;
            return Ok(Json(res));
        },
        ((query_res, room_id), _, _) = futures::future::select_all(queries) => {
            let (events, progress) = query_res?;
            let (joined, invited) = db.get_room_member_counts(&room_id).await?;
            let summary = RoomSummary {
                heroes: None,
                joined_member_count: joined,
                invited_member_count: invited,
            };
            batch.rooms.insert(room_id.clone(), progress + 1);
            res.rooms.get_or_insert_with(Default::default).join.insert(
                room_id.clone(),
                JoinedRoom {
                    summary,
                    timeline: Timeline {
                        events,
                        limited: false,
                        prev_batch: String::from("empty"),
                    },
                    state: State { events: Vec::new() },
                    ephemeral: Ephemeral {
                        events: db.get_all_ephemeral(&room_id).await?.into_iter().map(
                            |(k, v)| KvPair {
                                ty: k,
                                content: v,
                            }).collect()
                    },
                    account_data: AccountData { events: Vec::new() },
                }
            );
            db.set_batch(&next_batch_id, batch).await?;
            return Ok(Json(res));
        },
    };
}

#[get("/rooms/{room_id}/event/{event_id}")]
pub async fn get_event(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    path_args: Path<(String, String)>,
) -> Result<Json<Event>, Error> {
    let (room_id, event_id) = path_args.into_inner();
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    if db.get_membership(
        &user_id,
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
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    if db.get_membership(
        &user_id,
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
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    match db.get_membership(
        &user_id,
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
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();
    
    match db.get_membership(
        &user_id,
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
    let mut db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    let event = Event {
        event_content: EventContent::new(&event_type, event_content.into_inner())?,
        room_id: Some(room_id),
        sender: user_id,
        state_key: Some(state_key),
        redacts: None,
        unsigned: None,
        event_id: None,
        origin_server_ts: None,
    };

    let event_id = db.add_event(event).await?;

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
    let (room_id, event_type, _txn_id) = req.into_inner();
    let mut db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    let event = Event {
        event_content: EventContent::new(&event_type, event_content.into_inner())?,
        room_id: Some(room_id),
        sender: user_id,
        state_key: None,
        unsigned: None,
        redacts: None,
        event_id: None,
        origin_server_ts: None,
    };

    let event_id = db.add_event(event).await?;

    Ok(Json(SendEventResponse {
        event_id,
    }))
}
