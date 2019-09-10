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

pub async fn try_auth(db_client: &mut crate::db_pool::ClientGuard, token: Uuid) -> Result<i64, Error> {
    let query = db_client.prepare("SELECT id FROM access_tokens WHERE token = $1;").compat().await?;
    let mut rows = db_client.query(&query, &[&token]).compat();
    let row = rows.next().await.unwrap()?;
    let id = row.try_get("id")?;
    Ok(id)
}

#[tracing::instrument]
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

#[tracing::instrument]
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
    
    let mut db_client = cx.state().db_pool.get_client().await?;

    let get_user = db_client.prepare("SELECT * FROM users WHERE name=$1;").compat().await?;
    let mut rows = db_client.query(&get_user, &[&username]).compat(); 
    let user = rows.next().await.ok_or(Error::Forbidden)??;

    match argon2::verify_encoded(user.try_get("password_hash")?, password.as_bytes()) {
        Ok(true) => {},
        Ok(false) | Err(_) => return Err(Error::Forbidden),
    }

    let access_token = uuid::Uuid::new_v4();
    let device_id = request.device_id.unwrap_or(format!("{:08X}", rand::random::<u32>()));
    let id: i64 = user.try_get("id")?;
    let insert_token = db_client.prepare("INSERT INTO access_tokens(token, user_id, device_id) VALUES ($1, $2, $3);").compat().await?;
    db_client.execute(&insert_token, &[&access_token, &id, &device_id]).compat().await?;

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
    let mut db_client = cx.state().db_pool.get_client().await?;
    let stmt = db_client.prepare("DELETE FROM access_tokens WHERE token = $1;").compat().await?;
    db_client.execute(&stmt, &[&token]).compat().await?;
    Ok(response::json(json!({})))
}

pub async fn logout_all(cx: Context<Arc<ServerState>>) -> ClientResult {
    let token = get_access_token(&cx)?;
    let mut db_client = cx.state().db_pool.get_client().await?;
    let stmt = db_client.prepare(
        "WITH id AS (
            SELECT user_id FROM access_tokens WHERE token = $1
        )
        DELETE FROM access_tokens WHERE user_id IN id;").compat().await?;
    db_client.execute(&stmt, &[&token]).compat().await?;
    
    Ok(response::json(json!({})))
}

#[tracing::instrument]
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

    let mut db_client = cx.state().db_pool.get_client().await?;
    let stmt = db_client.prepare("INSERT INTO users(name, password_hash) VALUES ($1, $2); SELECT currval('users_id_seq');").compat().await?;
    let mut rows = db_client.query(&stmt, &[&request.username, &password_hash]).compat();
    let row = rows.next().await.expect("no users_id_seq value in table users")?;
    let id: i64 = row.get("currval");
    if request.inhibit_login {
        return Ok(response::json(json!({
            "user_id": request.username
        })));
    }
    
    let access_token = uuid::Uuid::new_v4();
    let device_id = request.device_id.unwrap_or(format!("{:08X}", rand::random::<u32>()));
    let insert_token = db_client.prepare("INSERT INTO access_tokens(token, user_id, device_id) VALUES ($1, $2, $3);").compat().await?;
    db_client.execute(&insert_token, &[&access_token, &id, &device_id]).compat().await?;

    let user_id = format!("@{}:{}", request.username, cx.state().config.domain);
    let access_token = format!("{}", access_token.to_hyphenated());

    Ok(response::json(json!({
        "user_id": user_id,
        "access_token": access_token,
        "device_id": device_id
    })))
}
