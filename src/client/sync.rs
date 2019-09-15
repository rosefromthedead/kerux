use futures::{
    compat::{Future01CompatExt, Stream01CompatExt},
    stream::{StreamExt, TryStreamExt},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::{
    collections::HashMap,
    sync::Arc
};
use tide::{response, querystring::ContextExt, Context};

use crate::{
    client::{
        auth::{get_access_token, try_auth},
        error::Error,
        ClientResult,
    },
    ServerState,
};

// Provided in URL query params
#[derive(Debug, Deserialize)]
struct SyncRequest {
    filter: String,
    since: String,
    full_state: bool,
    set_presence: String,
    timeout: u32,
}

#[derive(Debug, Serialize)]
struct SyncResponse {
    next_batch: String,
    rooms: Rooms,
    presence: Presence,
    account_data: AccountData,
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
    events: Vec<StateEvent>,
}

#[derive(Debug, Serialize)]
struct StateEvent {
    content: JsonValue,
    #[serde(rename = "type")]
    ty: String,
    event_id: String,
    sender: String,
    origin_server_ts: i64,
    unsigned: Option<JsonValue>,
    prev_content: Option<JsonValue>,
    state_key: String,
}

#[derive(Debug, Serialize)]
struct Timeline {
    events: Vec<RoomEvent>,
    limited: bool,
    prev_batch: String,
}

#[derive(Debug, Serialize)]
struct RoomEvent {
    content: JsonValue,
    #[serde(rename = "type")]
    ty: String,
    event_id: String,
    sender: String,
    origin_server_ts: i64,
    unsigned: Option<JsonValue>,
}

#[derive(Debug, Serialize)]
struct Ephemeral {
    events: Vec<Event>,
}

#[derive(Debug, Serialize)]
struct Event {
    content: JsonValue,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Debug, Serialize)]
pub struct AccountData {
    events: Vec<Event>,
}

#[derive(Debug, Serialize)]
pub struct InvitedRoom {
    invite_state: InviteState,
}

#[derive(Debug, Serialize)]
pub struct InviteState {
    events: Vec<StrippedState>,
}

#[derive(Debug, Serialize)]
pub struct StrippedState {
    content: JsonValue,
    state_key: String,
    #[serde(rename = "type")]
    ty: String,
    sender: String,
}

#[derive(Debug, Serialize)]
pub struct LeftRoom {
    state: State,
    timeline: Timeline,
    account_data: AccountData,
}

#[derive(Debug, Serialize)]
pub struct Presence {
    events: Vec<Event>,
}

pub async fn sync(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db_client = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let id = try_auth(&mut db_client, access_token).await?;

    let request = cx.url_query().map_err(|_| Error::InvalidParam(String::new()))?;
    tracing::debug!(req = tracing::field::debug(&request));

    let query = db_client.prepare("SELECT room_id FROM users_in_rooms WHERE user_id = $1;").compat().await?;
    let rows = db_client.query(&query, &[&id]).compat().map_ok(|row| -> i64 { row.get("room_id") });
    let rooms = rows.try_collect::<Vec<i64>>().await?;

    let query = db_client.prepare(
        "SELECT
            rooms.name AS room_name, content, type, event_id, sender, origin_server_ts, unsigned
            FROM room_events, rooms
            WHERE room_id IN $1;").compat().await?;
    let rows = db_client.query(&query, &[&rooms]).compat();
    let mut events_iter = rows
        .map(|row| -> Result<(String, RoomEvent), pg::Error> {
            let row = row?;
            Ok((row.get("room_name"), RoomEvent {
                content: row.get("content"),
                ty: row.get("type"),
                event_id: row.get("event_id"),
                sender: row.get("sender"),
                origin_server_ts: row.get("origin_server_ts"),
                unsigned: row.get("unsigned"),
            }))
        });
    
    let mut events = HashMap::<String, Vec<RoomEvent>>::new();
    while let Some(tuple) = events_iter.next().await {
        let (room_name, event) = tuple?;
        let events_list = events.entry(room_name).or_default();
        events_list.push(event);
    }

    Ok(response::json(json!({})))
}
