use futures::{
    compat::{Future01CompatExt, Stream01CompatExt},
    stream::StreamExt,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tide::{response, Context};
use uuid::Uuid;

use crate::{client::{error::Error, ClientResult}, ServerState};

#[derive(Debug, Deserialize)]
enum LoginType {
    #[serde(rename = "m.login.password")]
    Password,
}

pub fn get_access_token(cx: &Context<Arc<ServerState>>) -> Result<Uuid, Error> {
    if let Some(s) = cx.headers().get("Authorization") {
        let s: &str = s.to_str().map_err(|_| Error::MissingToken)?;
        if !s.starts_with("Bearer ") {
            return Err(Error::MissingToken);
        }
        let token = s.trim_start_matches("Bearer ").parse().map_err(|_| Error::UnknownToken)?;
        Ok(token)
    } else if let Some(pair) = cx.uri().query().ok_or(Error::MissingToken)?.split('&').find(|pair| pair.starts_with("access_token")) {
        let token = pair.trim_start_matches("access_token=").parse().map_err(|_| Error::UnknownToken)?;
        Ok(token)
    } else {
        Err(Error::MissingToken)
    }
}

pub async fn get_supported_login_types(_cx: Context<Arc<ServerState>>) -> ClientResult {
    //TODO: allow config
    Ok(response::json(json!({
        "flows": [
            {
                "type": "m.login.password"
            }
        ]
    })))
}

pub async fn login(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    enum Identifier {
        #[serde(rename = "m.id.user")]
        Username {
            user: String,
        },
        #[serde(rename = "m.id.thirdparty")]
        ThirdParty {
            medium: String,
            address: String,
        },
        #[serde(rename = "m.id.phone")]
        Phone {
            country: String,
            phone: String,
        },
    }
    #[derive(Debug, Deserialize)]
    struct LoginRequest {
        #[serde(rename = "type")]
        login_type: LoginType,
        identifier: Identifier,
        password: Option<String>,
        token: Option<String>,
        device_id: Option<String>,
        initial_device_display_name: String,
    }
    let request: LoginRequest = cx.body_json().await?;
    tracing::debug!(req = tracing::field::debug(&request));

    let username = match request.identifier {
        Identifier::Username { user } => user,
        _ => return Err(Error::Unimplemented),
    };
    let password = request.password.ok_or(Error::Unimplemented)?;
    
    let mut db = cx.state().db_pool.get_client().await?;
    if !db.verify_password(&username, &password).await? {
        return Err(Error::Forbidden);
    }

    let device_id = request.device_id.unwrap_or(format!("{:08X}", rand::random::<u32>()));
    let access_token = db.create_access_token(&username, &device_id).await?;

    let user_id = format!("@{}:{}", username, cx.state().config.domain);
    let access_token = format!("{}", access_token.to_hyphenated());

    Ok(response::json(json!({
        "user_id": user_id,
        "access_token": access_token,
        "device_id": device_id,
    })))
}

pub async fn logout(cx: Context<Arc<ServerState>>) -> ClientResult {
    let token = get_access_token(&cx)?;
    let mut db = cx.state().db_pool.get_client().await?;
    db.delete_access_token(token).await?;
    Ok(response::json(json!({})))
}

pub async fn logout_all(cx: Context<Arc<ServerState>>) -> ClientResult {
    let token = get_access_token(&cx)?;
    let mut db = cx.state().db_pool.get_client().await?;
    db.delete_all_access_tokens(token).await?;    
    Ok(response::json(json!({})))
}

pub async fn register(mut cx: Context<Arc<ServerState>>) -> ClientResult {
    #[derive(Debug, Deserialize)]
    struct RegisterRequest {
        auth: serde_json::Value,
        bind_email: bool,
        bind_msisdn: bool,
        username: String,
        password: String,
        device_id: Option<String>,
        initial_device_display_name: String,
        inhibit_login: bool,
    }
    let kind = cx.param::<String>("user_id")?;
    match &*kind {
        "user" => {},
        "guest" => return Err(Error::Unimplemented),
        _ => return Err(Error::InvalidParam(kind)),
    }
    let request: RegisterRequest = cx.body_json().await?;
    tracing::debug!(req = tracing::field::debug(&request));

    let salt: [u8; 16] = rand::random();
    let password_hash = argon2::hash_encoded(request.password.as_bytes(), &salt, &Default::default())?;

    let mut db = cx.state().db_pool.get_client().await?;
    db.create_user(&request.username, Some(&password_hash)).await?;
    if request.inhibit_login {
        return Ok(response::json(json!({
            "user_id": request.username
        })));
    }
    
    let device_id = request.device_id.unwrap_or(format!("{:08X}", rand::random::<u32>()));
    let access_token = db.create_access_token(&request.username, &device_id).await?;

    let user_id = format!("@{}:{}", request.username, cx.state().config.domain);
    let access_token = format!("{}", access_token.to_hyphenated());

    Ok(response::json(json!({
        "user_id": user_id,
        "access_token": access_token,
        "device_id": device_id
    })))
}
