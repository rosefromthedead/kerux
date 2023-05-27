use actix_web::{
    post,
    web::{Data, Json, Path},
};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use std::{collections::HashMap, sync::Arc};
use tracing::{field::Empty, instrument, Level, Span};

use crate::{
    client_api::auth::AccessToken,
    error::{Error, ErrorKind},
    events::{room, EventContent},
    storage::UserProfile,
    util::{storage::NewEvent, MatrixId, StorageExt},
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
#[instrument(skip_all, fields(username = Empty), err = Level::DEBUG)]
pub async fn create_room(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req: Json<CreateRoomRequest>,
) -> Result<Json<JsonValue>, Error> {
    let req = req.into_inner();
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(ErrorKind::UnknownToken)?;
    Span::current().record("username", &username.as_str());
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();

    let room_version = req.room_version.unwrap_or("4".to_string());
    if room_version != "4" {
        return Err(ErrorKind::UnsupportedRoomVersion.into());
    }

    let room_id = format!("!{:016X}:{}", rand::random::<i64>(), state.config.domain);

    db.add_event(
        &room_id,
        NewEvent {
            event_content: EventContent::Create(room::Create {
                creator: user_id.clone(),
                room_version: Some(room_version),
                predecessor: None,
                extra: match req.creation_content {
                    Some(v) => v,
                    None => HashMap::new(),
                },
            }),
            sender: user_id.clone(),
            state_key: Some(String::new()),
            redacts: None,
            unsigned: None,
        },
        &state.state_resolver,
    )
    .await?;

    let creator_join = {
        let UserProfile {
            avatar_url,
            displayname,
        } = db.get_profile(&username).await?.unwrap();
        room::Member {
            avatar_url,
            displayname,
            membership: room::Membership::Join,
            is_direct: req.is_direct,
        }
    };
    db.add_event(
        &room_id,
        NewEvent {
            event_content: EventContent::Member(creator_join),
            sender: user_id.clone(),
            state_key: Some(user_id.clone_inner()),
            redacts: None,
            unsigned: None,
        },
        &state.state_resolver,
    )
    .await?;

    // TODO: default power levels a bit of a mess
    db.add_event(
        &room_id,
        NewEvent {
            event_content: EventContent::PowerLevels(
                req.power_level_content_override.unwrap_or_default(),
            ),
            sender: user_id.clone(),
            state_key: Some(String::new()),
            redacts: None,
            unsigned: None,
        },
        &state.state_resolver,
    )
    .await?;

    let (join_rule, history_visibility, guest_access) = {
        use room::{GuestAccessType::*, HistoryVisibilityType::*, JoinRule::*};
        let preset = req.preset.unwrap_or(match req.visibility {
            RoomVisibility::Private => Preset::PrivateChat,
            RoomVisibility::Public => Preset::PublicChat,
        });
        match preset {
            Preset::PrivateChat | Preset::TrustedPrivateChat => (Invite, Shared, CanJoin),
            Preset::PublicChat => (Public, Shared, Forbidden),
        }
    };
    db.add_event(
        &room_id,
        NewEvent {
            event_content: EventContent::JoinRules(room::JoinRules { join_rule }),
            sender: user_id.clone(),
            state_key: Some(String::new()),
            redacts: None,
            unsigned: None,
        },
        &state.state_resolver,
    )
    .await?;
    db.add_event(
        &room_id,
        NewEvent {
            event_content: EventContent::HistoryVisibility(room::HistoryVisibility {
                history_visibility,
            }),
            sender: user_id.clone(),
            state_key: Some(String::new()),
            redacts: None,
            unsigned: None,
        },
        &state.state_resolver,
    )
    .await?;
    db.add_event(
        &room_id,
        NewEvent {
            event_content: EventContent::GuestAccess(room::GuestAccess {
                guest_access: Some(guest_access),
            }),
            sender: user_id.clone(),
            state_key: Some(String::new()),
            redacts: None,
            unsigned: None,
        },
        &state.state_resolver,
    )
    .await?;

    for event in req.initial_state.into_iter().flatten() {
        db.add_event(
            &room_id,
            NewEvent {
                event_content: EventContent::new(&event.ty, event.content)?,
                sender: user_id.clone(),
                state_key: Some(event.state_key),
                redacts: None,
                unsigned: None,
            },
            &state.state_resolver,
        )
        .await?;
    }

    if let Some(name) = req.name {
        db.add_event(
            &room_id,
            NewEvent {
                event_content: EventContent::Name(room::Name { name: Some(name) }),
                sender: user_id.clone(),
                state_key: Some(String::new()),
                redacts: None,
                unsigned: None,
            },
            &state.state_resolver,
        )
        .await?;
    }

    if let Some(topic) = req.topic {
        db.add_event(
            &room_id,
            NewEvent {
                event_content: EventContent::Topic(room::Topic { topic: Some(topic) }),
                sender: user_id.clone(),
                state_key: Some(String::new()),
                redacts: None,
                unsigned: None,
            },
            &state.state_resolver,
        )
        .await?;
    }

    for invitee in req.invite.into_iter().flatten() {
        db.add_event(
            &room_id,
            NewEvent {
                event_content: EventContent::Member(room::Member {
                    avatar_url: None,
                    displayname: None,
                    membership: room::Membership::Invite,
                    is_direct: req.is_direct,
                }),
                sender: user_id.clone(),
                state_key: Some(invitee),
                redacts: None,
                unsigned: None,
            },
            &state.state_resolver,
        )
        .await?;
    }

    tracing::info!(room_id = room_id.as_str(), "Created room");

    Ok(Json(json!({ "room_id": room_id })))
}

#[derive(Deserialize)]
pub struct InviteRequest {
    user_id: MatrixId,
}

#[post("/rooms/{room_id}/invite")]
#[instrument(skip(state, token, req), fields(username = Empty), err = Level::DEBUG)]
pub async fn invite(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    Path(room_id): Path<String>,
    req: Json<InviteRequest>,
) -> Result<Json<JsonValue>, Error> {
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(ErrorKind::UnknownToken)?;
    Span::current().record("username", &username.as_str());
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();
    let invitee = req.into_inner().user_id;
    let invitee_profile = db
        .get_profile(&invitee.localpart())
        .await?
        .unwrap_or_default();

    let invite_event = NewEvent {
        event_content: EventContent::Member(room::Member {
            avatar_url: invitee_profile.avatar_url,
            displayname: invitee_profile.displayname,
            membership: room::Membership::Invite,
            is_direct: Some(false),
        }),
        sender: user_id.clone(),
        state_key: Some(invitee.clone_inner()),
        redacts: None,
        unsigned: None,
    };

    db.add_event(&room_id, invite_event, &state.state_resolver)
        .await?;

    Ok(Json(json!({})))
}

#[post("/join/{room_id_or_alias}")]
#[instrument(skip(state, token), fields(username = Empty), err = Level::DEBUG)]
pub async fn join_by_id_or_alias(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    Path(room_id_or_alias): Path<String>,
) -> Result<Json<JsonValue>, Error> {
    //TODO: implement server_name and third_party_signed args, and room aliases
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(ErrorKind::UnknownToken)?;
    Span::current().record("username", &username.as_str());
    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();
    let profile = db.get_profile(&username).await?.unwrap_or_default();

    let event = NewEvent {
        event_content: EventContent::Member(room::Member {
            avatar_url: profile.avatar_url,
            displayname: profile.displayname,
            membership: room::Membership::Join,
            is_direct: Some(false),
        }),
        sender: user_id.clone(),
        state_key: Some(user_id.to_string()),
        redacts: None,
        unsigned: None,
    };

    db.add_event(&room_id_or_alias, event, &state.state_resolver)
        .await?;

    Ok(Json(serde_json::json!({ "room_id": room_id_or_alias })))
}
