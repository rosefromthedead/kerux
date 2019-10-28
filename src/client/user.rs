use percent_encoding::percent_decode_str;
use serde_json::json;
use std::sync::Arc;
use tide::{response, Context};

use crate::{
    client::{
        auth::get_access_token,
        error::Error,
        ClientResult
    },
    ServerState,
};

pub async fn get_avatar_url(cx: Context<Arc<ServerState>>) -> ClientResult {
    let user_param = cx.param::<String>("user_id")?;
    let user_id = percent_decode_str(&user_param).decode_utf8()
        .map_err(|e| format!("{}", e)).map_err(Error::InvalidParam)?;
    let user_id = user_id.trim_start_matches('@');
    let (username, domain) = {
        let mut iter = user_id.split(':');
        let user_id = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (user_id, domain)
    };
    if domain != cx.state().config.domain {
        return Err(Error::Unimplemented);
    }

    let mut db = cx.state().db_pool.get_client().await?;
    let avatar_url = match db.get_profile(user_id).await?.0 {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(response::json(json!({
        "avatar_url": avatar_url
    })))
}

pub async fn set_avatar_url(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let user_id = db.try_auth(access_token).await?;
    let body: serde_json::Value = cx.body_json().await?;
    let avatar_url = body
        .get("avatar_url").ok_or(Error::BadJson(String::from("no avatar_url field")))?
        .as_str().ok_or(Error::BadJson(String::from("avatar_url should be a string")))?;
    db.set_avatar_url(&user_id, avatar_url).await?;
    Ok(response::json(json!({})))
}

pub async fn get_display_name(cx: Context<Arc<ServerState>>) -> ClientResult {
    let user_param = cx.param::<String>("user_id")?;
    let user_id = percent_decode_str(&user_param).decode_utf8()
        .map_err(|e| format!("{}", e)).map_err(Error::InvalidParam)?;
    let user_id = user_id.trim_start_matches('@');
    let (username, domain) = {
        let mut iter = user_id.split(':');
        let username = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (username, domain)
    };
    if domain != cx.state().config.domain {
        return Err(Error::Unimplemented);
    }

    let mut db = cx.state().db_pool.get_client().await?;
    let display_name = match db.get_profile(user_id).await?.1 {
        Some(v) => v,
        None => return Err(Error::NotFound),
    };

    Ok(response::json(json!({
        "displayname": display_name
    })))
}

pub async fn set_display_name(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let user_id = db.try_auth(access_token).await?;
    let body: serde_json::Value = cx.body_json().await?;
    let display_name = body
        .get("displayname").ok_or(Error::BadJson(String::from("no displayname field")))?
        .as_str().ok_or(Error::BadJson(String::from("displayname should be a string")))?;
    db.set_display_name(&user_id, &display_name).await?;
    Ok(response::json(json!({})))
}

pub async fn get_profile(cx: Context<Arc<ServerState>>) -> ClientResult {
    let user_param: String = cx.param("user_id")?;
    let user_id = percent_decode_str(&user_param).decode_utf8()
        .map_err(|e| format!("{}", e)).map_err(Error::InvalidParam)?;
    let user_id = user_id.trim_start_matches('@');
    let (username, domain) = {
        let mut iter = user_id.split(':');
        let username = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (username, domain)
    };
    if domain != cx.state().config.domain {
        return Err(Error::Unimplemented);
    }

    let mut db = cx.state().db_pool.get_client().await?;
    let (avatar_url, display_name) = db.get_profile(user_id).await?;
    let mut response = serde_json::Map::new();
    if let Some(v) = avatar_url {
        response.insert("avatar_url".into(), v.into());
    }
    if let Some(v) = display_name {
        response.insert("display_name".into(), v.into());
    }

    Ok(response::json(response))
}
