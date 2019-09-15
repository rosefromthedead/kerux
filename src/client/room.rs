use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::{
    collections::HashMap,
    sync::Arc,
};
use tide::{response, Context};

use crate::{
    client::{
        auth::{get_access_token, try_auth},
        error::Error,
        ClientResult,
    },
    ServerState,
};

#[derive(Deserialize)]
struct CreateRoomRequest {
    visibility: RoomVisibility,
    room_alias_name: Option<String>,
    name: String,
    topic: Option<String>,
    invite: Option<Vec<String>>,
    invite_3pid: Option<Vec<Invite3pid>>,
    room_version: Option<String>,
    creation_content: Option<HashMap<String, JsonValue>>,
    initial_state: Vec<StateEvent>,
    preset: Option<Preset>,
    is_direct: Option<bool>,
    power_level_content_override: Option<crate::events::room::PowerLevels>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
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

pub async fn create_room(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db_client = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let id = try_auth(&mut db_client, access_token).await?;

    Ok(response::json(()))
}
