use futures::{
    compat::{Future01CompatExt, Stream01CompatExt},
    stream::StreamExt,
};
use percent_encoding::percent_decode_str;
use serde_json::json;
use std::sync::Arc;
use tide::{response, Context};

use crate::{
    client::{
        auth::{get_access_token, try_auth},
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
        let username = iter.next().unwrap();
        let domain = iter.next().ok_or(Error::Unknown("Domain must be provided".to_string()))?;
        (username, domain)
    };
    if domain != cx.state().config.domain {
        return Err(Error::Unimplemented);
    }
    let mut db_client = cx.state().db_pool.get_client().await?;
    let query = db_client.prepare("SELECT avatar_url FROM users WHERE name = $1;").compat().await?;
    let mut rows = db_client.query(&query, &[&username]).compat();
    let row = rows.next().await.ok_or(Error::NotFound)??;
    let avatar_url: Option<&str> = row.try_get("avatar_url")?;
    Ok(response::json(json!({
        "avatar_url": avatar_url
    })))
}

pub async fn set_avatar_url(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db_client = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let id = try_auth(&mut db_client, access_token).await?;
    let body: serde_json::Value = cx.body_json().await?;
    let avatar_url = body
        .get("avatar_url").ok_or(Error::BadJson(String::from("no avatar_url field")))?
        .as_str().ok_or(Error::BadJson(String::from("avatar_url should be a string")))?;
    let stmt = db_client.prepare("UPDATE users SET avatar_url = $1 WHERE id = $2;").compat().await?;
    db_client.execute(&stmt, &[&avatar_url, &id]).compat().await?;
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
    let mut db_client = cx.state().db_pool.get_client().await?;
    let query = db_client.prepare("SELECT display_name FROM users WHERE name = $1;").compat().await?;
    let mut rows = db_client.query(&query, &[&username]).compat();
    let row = rows.next().await.ok_or(Error::NotFound)??;
    let display_name: Option<&str> = row.try_get("display_name")?;
    Ok(response::json(json!({
        "displayname": display_name
    })))
}

pub async fn set_display_name(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    let mut db_client = cx.state().db_pool.get_client().await?;
    let access_token = get_access_token(&cx)?;
    let id = try_auth(&mut db_client, access_token).await?;
    let body: serde_json::Value = cx.body_json().await?;
    let display_name = body
        .get("displayname").ok_or(Error::BadJson(String::from("no displayname field")))?
        .as_str().ok_or(Error::BadJson(String::from("displayname should be a string")))?;
    let stmt = db_client.prepare("UPDATE users SET display_name = $1 WHERE id = $2;").compat().await?;
    db_client.execute(&stmt, &[&display_name, &id]).compat().await?;
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
    let mut db_client = cx.state().db_pool.get_client().await?;
    let query = db_client.prepare("SELECT display_name, avatar_url FROM users WHERE name = $1;").compat().await?;
    let mut rows = db_client.query(&query, &[&username]).compat();
    let row = rows.next().await.ok_or(Error::NotFound)??;
    let avatar_url: Option<&str> = row.try_get("avatar_url")?;
    let display_name: Option<&str> = row.try_get("display_name")?;
    Ok(response::json(json!({
        "avatar_url": avatar_url,
        "displayname": display_name
    })))
}
