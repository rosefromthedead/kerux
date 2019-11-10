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
        auth::get_access_token,
        error::Error,
        ClientResult,
    },
    events::room::Membership,
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
struct AccountData {
    events: Vec<Event>,
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
    events: Vec<Event>,
}

pub async fn sync(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let username = db.try_auth(access_token).await?;
    let user_id = format!("@{}:{}", username, cx.state().config.domain);

    let request = cx.url_query().map_err(|_| Error::InvalidParam(String::new()))?;
    tracing::debug!(req = tracing::field::debug(&request));

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
                join.insert(room_id, JoinedRoom {
                    summary,
                    state,
                });
            },
            Membership::Invite => {},
            Membership::Leave => {}
        }
    }
    let rooms = Rooms { join, invite, leave };

    Ok(response::json(SyncResponse {
        next_batch: String::new(),
        rooms: Some(rooms),
        presence: None,
        account_data: None,
    }))
}
