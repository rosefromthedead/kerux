use actix_web::{
    web::{Data, Json, Path},
    get, put,
};
use serde_json::{json, Value as JsonValue};
use std::sync::Arc;

use crate::{
    client::{
        auth::AccessToken,
        error::Error,
    },
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

    let mut db = state.db_pool.get_client().await?;
    let avatar_url = match db.get_profile(username).await?.0 {
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
    let mut db = state.db_pool.get_client().await?;
    let user_id = db.try_auth(token.0).await?;
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

    let mut db = state.db_pool.get_client().await?;
    let display_name = match db.get_profile(username).await?.1 {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(Json(json!({
        "displayname": display_name
    })))
}

#[put("/profile/{user_id}/displayname")]
pub async fn set_display_name(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    req_id: Path<String>,
    body: Json<JsonValue>
) -> Result<Json<()>, Error> {
    let mut db = state.db_pool.get_client().await?;
    let user_id = db.try_auth(token.0).await?;
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

    let mut db = state.db_pool.get_client().await?;
    let (avatar_url, display_name) = db.get_profile(&username).await?;
    let mut response = serde_json::Map::new();
    if let Some(v) = avatar_url {
        response.insert("avatar_url".into(), v.into());
    }
    if let Some(v) = display_name {
        response.insert("display_name".into(), v.into());
    }

    Ok(Json(response.into()))
}
