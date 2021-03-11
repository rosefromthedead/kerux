use actix_web::{
    web::{Data, Json, Path},
    get, post, put,
};
use tracing::{Level, Span, instrument, field::Empty};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::sync::Arc;

use crate::{
    client::{
        auth::AccessToken,
        error::Error,
    },
    storage::{Storage, StorageManager, UserProfile},
    util::MatrixId,
    ServerState,
};

#[get("/profile/{user_id}/avatar_url")]
#[instrument(skip(state), err = Level::DEBUG)]
pub async fn get_avatar_url(
    state: Data<Arc<ServerState>>,
    Path(user_id): Path<MatrixId>
) -> Result<Json<JsonValue>, Error> {
    if user_id.domain() != state.config.domain {
        return Err(Error::Unimplemented);
    }

    let db = state.db_pool.get_handle().await?;
    let avatar_url = match db.get_profile(&user_id.localpart()).await?.unwrap().avatar_url {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(Json(json!({
        "avatar_url": avatar_url
    })))
}

#[put("/profile/{user_id}/avatar_url")]
#[instrument(skip(state, token, body), fields(username = Empty), err = Level::DEBUG)]
pub async fn set_avatar_url(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    Path(req_id): Path<MatrixId>,
    body: Json<JsonValue>
) -> Result<Json<()>, Error> {
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    Span::current().record("username", &username.as_str());

    if req_id.localpart() != username {
        return Err(Error::Forbidden);
    }
    if req_id.domain() != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let avatar_url = body
        .get("avatar_url").ok_or(Error::BadJson(String::from("no avatar_url field")))?
        .as_str().ok_or(Error::BadJson(String::from("avatar_url should be a string")))?;
    db.set_avatar_url(&username, avatar_url).await?;
    Ok(Json(()))
}

#[get("/profile/{user_id}/displayname")]
#[instrument(skip(state), err = Level::DEBUG)]
pub async fn get_display_name(
    state: Data<Arc<ServerState>>,
    Path(user_id): Path<MatrixId>
) -> Result<Json<JsonValue>, Error> {
    if user_id.domain() != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let db = state.db_pool.get_handle().await?;
    let displayname = match db.get_profile(&user_id.localpart()).await?.unwrap().displayname {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(Json(json!({
        "displayname": displayname
    })))
}

#[put("/profile/{user_id}/displayname")]
#[instrument(skip(state, token, body), fields(username = Empty), err = Level::DEBUG)]
pub async fn set_display_name(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    Path(req_id): Path<MatrixId>,
    body: Json<JsonValue>
) -> Result<Json<()>, Error> {
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    Span::current().record("username", &username.as_str());

    if req_id.localpart() != username {
        return Err(Error::Forbidden);
    }
    if req_id.domain() != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let display_name = body
        .get("displayname").ok_or(Error::BadJson(String::from("no displayname field")))?
        .as_str().ok_or(Error::BadJson(String::from("displayname should be a string")))?;
    db.set_display_name(&username, &display_name).await?;
    Ok(Json(()))
}

#[get("/profile/{user_id}")]
#[instrument(skip(state), err = Level::DEBUG)]
pub async fn get_profile(
    state: Data<Arc<ServerState>>,
    Path(user_id): Path<MatrixId>
) -> Result<Json<JsonValue>, Error> {
    if user_id.domain() != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let db = state.db_pool.get_handle().await?;
    let UserProfile { avatar_url, displayname } = db.get_profile(&user_id.localpart()).await?.unwrap();
    let mut response = serde_json::Map::new();
    if let Some(v) = avatar_url {
        response.insert("avatar_url".into(), v.into());
    }
    if let Some(v) = displayname {
        response.insert("displayname".into(), v.into());
    }

    Ok(Json(response.into()))
}

#[derive(Deserialize)]
pub struct UserDirSearchRequest {
    search_term: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
pub struct UserDirSearchResponse {
    results: Vec<User>,
    limited: bool,
}

#[derive(Serialize)]
struct User {
    user_id: MatrixId,
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

//TODO: actually implement this
#[post("/user_directory/search")]
#[instrument(skip_all, err = Level::DEBUG)]
pub async fn search_user_directory(
    state: Data<Arc<ServerState>>,
    req: Json<UserDirSearchRequest>,
) -> Result<Json<UserDirSearchResponse>, Error> {
    let req = req.into_inner();
    let db = state.db_pool.get_handle().await?;
    let searched_user = MatrixId::new(&req.search_term, &state.config.domain)
        .map_err(|e| Error::Unknown(e.to_string()))?;
    let user_profile = db.get_profile(searched_user.localpart()).await?;
    match user_profile {
        Some(p) => Ok(Json(UserDirSearchResponse {
            results: vec![User {
                user_id: searched_user,
                avatar_url: p.avatar_url,
                display_name: p.displayname,
            }],
            limited: false,
        })),
        None => Ok(Json(UserDirSearchResponse {
            results: Vec::new(),
            limited: false,
        })),
    }
}

#[derive(Serialize)]
pub struct Get3pidsResponse {
    threepids: Vec<Threepid>,
}

#[derive(Serialize)]
struct Threepid {
    medium: Medium,
    address: String,
    validated_at: u64,
    added_at: u64,
}

#[derive(Serialize)]
pub enum Medium {
    Email,
    // Phone number, including calling code
    Msisdn,
}

#[get("/account/3pid")]
#[instrument(skip_all, err = Level::DEBUG)]
pub async fn get_3pids(
    _state: Data<Arc<ServerState>>,
    _token: AccessToken,
) -> Result<Json<Get3pidsResponse>, Error> {
    //TODO: implement
    Ok(Json(Get3pidsResponse {
        threepids: Vec::new(),
    }))
}
