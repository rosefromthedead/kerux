use actix_web::{
    web::{Data, Json, Path},
    get, post, put,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::sync::Arc;

use crate::{
    client::{
        auth::AccessToken,
        error::Error,
    },
    storage::{Storage, StorageManager, UserProfile},
    ServerState,
};

#[get("/profile/{user_id}/avatar_url")]
pub async fn get_avatar_url(
    state: Data<Arc<ServerState>>,
    user_id: Path<String>
) -> Result<Json<JsonValue>, Error> {
    let (username, domain) = {
        let mut iter = user_id.trim_start_matches('@').split(':');
        let user_id = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (user_id, domain)
    };
    if domain != state.config.domain {
        return Err(Error::Unimplemented);
    }

    let mut db = state.db_pool.get_handle().await?;
    let avatar_url = match db.get_profile(username).await?.unwrap().avatar_url {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(Json(json!({
        "avatar_url": avatar_url
    })))
}

#[put("/profile/{user_id}/avatar_url")]
pub async fn set_avatar_url(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req_id: Path<String>,
    body: Json<JsonValue>
) -> Result<Json<()>, Error> {
    let mut db = state.db_pool.get_handle().await?;
    let user_id = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    if *req_id != user_id {
        return Err(Error::Forbidden);
    }

    let (username, domain) = {
        let mut iter = user_id.trim_start_matches('@').split(':');
        let user_id = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (user_id, domain)
    };
    if domain != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let avatar_url = body
        .get("avatar_url").ok_or(Error::BadJson(String::from("no avatar_url field")))?
        .as_str().ok_or(Error::BadJson(String::from("avatar_url should be a string")))?;
    db.set_avatar_url(&username, avatar_url).await?;
    Ok(Json(()))
}

#[get("/profile/{user_id}/displayname")]
pub async fn get_display_name(
    state: Data<Arc<ServerState>>,
    user_id: Path<String>
) -> Result<Json<JsonValue>, Error> {
    let (username, domain) = {
        let mut iter = user_id.trim_start_matches('@').split(':');
        let username = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (username, domain)
    };
    if domain != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let mut db = state.db_pool.get_handle().await?;
    let displayname = match db.get_profile(username).await?.unwrap().displayname {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(Json(json!({
        "displayname": displayname
    })))
}

#[put("/profile/{user_id}/displayname")]
pub async fn set_display_name(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req_id: Path<String>,
    body: Json<JsonValue>
) -> Result<Json<()>, Error> {
    let mut db = state.db_pool.get_handle().await?;
    let user_id = db.try_auth(token.0).await?.ok_or(Error::UnknownToken)?;
    if *req_id != user_id {
        return Err(Error::Forbidden);
    }

    let (username, domain) = {
        let mut iter = user_id.trim_start_matches('@').split(':');
        let user_id = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (user_id, domain)
    };
    if domain != state.config.domain {
        return Err(Error::Unknown("User does not live on this homeserver".to_string()));
    }

    let display_name = body
        .get("displayname").ok_or(Error::BadJson(String::from("no displayname field")))?
        .as_str().ok_or(Error::BadJson(String::from("displayname should be a string")))?;
    db.set_display_name(&username, &display_name).await?;
    Ok(Json(()))
}

#[get("/profile/{user_id}")]
pub async fn get_profile(
    state: Data<Arc<ServerState>>,
    user_id: Path<String>
) -> Result<Json<JsonValue>, Error> {
    let (username, domain) = {
        let mut iter = user_id.trim_start_matches('@').split(':');
        let username = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (username, domain)
    };
    if domain != state.config.domain {
        return Err(Error::Unimplemented);
    }

    let mut db = state.db_pool.get_handle().await?;
    let UserProfile { avatar_url, displayname } = db.get_profile(&username).await?.unwrap();
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
    user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

//TODO: actually implement this
#[post("/user_directory/search")]
pub async fn search_user_directory(
    state: Data<Arc<ServerState>>,
    req: Json<UserDirSearchRequest>,
) -> Result<Json<UserDirSearchResponse>, Error> {
    let req = req.into_inner();
    let mut db = state.db_pool.get_handle().await?;
    let user_profile = db.get_profile(&req.search_term).await?;
    match user_profile {
        Some(p) => Ok(Json(UserDirSearchResponse {
            results: vec![User {
                user_id: req.search_term.clone(),
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
