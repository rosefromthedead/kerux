use actix_web::{
    dev::Payload,
    web::{Data, Json},
    get, post, HttpRequest, FromRequest,
};
use tracing::{instrument, Level, span::Span};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{convert::TryFrom, sync::Arc};
use uuid::Uuid;

use crate::{client::error::Error, storage::{Storage, StorageManager}, util::MatrixId, ServerState};

#[derive(Debug, Deserialize)]
enum LoginType {
    #[serde(rename = "m.login.password")]
    Password,
}

#[derive(Debug)]
pub struct AccessToken(pub Uuid);

impl FromRequest for AccessToken {
    type Error = Error;
    type Future = futures::future::Ready<Result<Self, Self::Error>>;
    type Config = ();
    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let res = (|| {
            if let Some(s) = req.headers().get("Authorization") {
                let s: &str = s.to_str().map_err(|_| Error::MissingToken)?;
                if !s.starts_with("Bearer ") {
                    return Err(Error::MissingToken);
                }
                let token = s.trim_start_matches("Bearer ").parse().map_err(|_| Error::UnknownToken)?;
                Ok(token)
            } else if let Some(pair) = req.uri().query().ok_or(Error::MissingToken)?.split('&').find(|pair| pair.starts_with("access_token")) {
                let token = pair.trim_start_matches("access_token=").parse().map_err(|_| Error::UnknownToken)?;
                Ok(token)
            } else {
                Err(Error::MissingToken)
            }
        })();
        match res {
            Ok(token) => futures::future::ok(AccessToken(token)),
            Err(e) => futures::future::err(e),
        }
    }
}

#[get("/login")]
#[instrument]
pub async fn get_supported_login_types() -> Json<serde_json::Value> {
    //TODO: allow config
    Json(json!({
        "flows": [
            {
                "type": "m.login.password"
            }
        ]
    }))
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    #[serde(rename = "type")]
    login_type: LoginType,
    identifier: Identifier,
    password: Option<String>,
    token: Option<String>,
    device_id: Option<String>,
    initial_device_display_name: String,
}

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

#[derive(Serialize)]
pub struct LoginResponse {
    user_id: MatrixId,
    access_token: String,
    device_id: String,
    //TODO: This is deprecated, but Fractal is the only client that doesn't require it. Remove it
    // once all the other clients have updated to current spec
    home_server: String,
}

#[post("/login")]
#[instrument(skip_all, err = Level::DEBUG)]
pub async fn login(
    state: Data<Arc<ServerState>>,
    req: Json<LoginRequest>,
) -> Result<Json<LoginResponse>, Error> {
    let req = req.into_inner();

    let username = match req.identifier {
        Identifier::Username { user } => {
            let res = MatrixId::try_from(&*user);
            match res {
                Ok(mxid) => mxid.localpart().to_string(),
                Err(_) => user,
            }
        },
        _ => return Err(Error::Unimplemented),
    };
    let password = req.password.ok_or(Error::Unimplemented)?;

    let db = state.db_pool.get_handle().await?;
    if !db.verify_password(&username, &password).await? {
        return Err(Error::Forbidden);
    }

    let device_id = req.device_id.unwrap_or(format!("{:08X}", rand::random::<u32>()));
    let access_token = db.create_access_token(&username, &device_id).await?;

    tracing::info!(username = username.as_str(), "User logged in");

    let user_id = MatrixId::new(&username, &state.config.domain).unwrap();
    let access_token = format!("{}", access_token.to_hyphenated());

    Ok(Json(LoginResponse {
        user_id,
        access_token,
        device_id,
        home_server: state.config.domain.clone(),
    }))
}

#[post("/logout")]
#[instrument(skip(state), err = Level::DEBUG)]
pub async fn logout(state: Data<Arc<ServerState>>, token: AccessToken) -> Result<Json<()>, Error> {
    let db = state.db_pool.get_handle().await?;
    db.delete_access_token(token.0).await?;
    Ok(Json(()))
}

#[post("/logout/all")]
#[instrument(skip(state), err = Level::DEBUG)]
pub async fn logout_all(state: Data<Arc<ServerState>>, token: AccessToken) -> Result<Json<()>, Error> {
    let db = state.db_pool.get_handle().await?;
    db.delete_all_access_tokens(token.0).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    auth: serde_json::Value,
    bind_email: bool,
    bind_msisdn: bool,
    username: String,
    password: String,
    device_id: Option<String>,
    initial_device_display_name: String,
    inhibit_login: bool,
}

#[post("/register")]
#[instrument(skip_all, fields(username = ""), err = Level::DEBUG)]
pub async fn register(
    state: Data<Arc<ServerState>>,
    req: Json<RegisterRequest>,
    http_req: HttpRequest
) -> Result<Json<serde_json::Value>, Error> {
    let req = req.into_inner();
    let query_string = http_req.query_string();
    match query_string.split('&').find(|s| s.starts_with("kind=")) {
        Some("kind=user") => {},
        Some("kind=guest") => return Err(Error::Unimplemented),
        Some(x) => return Err(Error::InvalidParam(x.to_string())),
        None => return Err(Error::MissingParam("kind".to_string())),
    }

    Span::current().record("username", &&*req.username);

    let user_id = MatrixId::new(&req.username, &state.config.domain)
        .map_err(|e| Error::BadJson(format!("{}", e)))?;

    let salt: [u8; 16] = rand::random();
    let password_hash = argon2::hash_encoded(req.password.as_bytes(), &salt, &Default::default())?;

    let db = state.db_pool.get_handle().await?;
    db.create_user(&user_id.localpart(), Some(&password_hash)).await?;
    if req.inhibit_login {
        return Ok(Json(json!({
            "user_id": req.username
        })));
    }

    let device_id = req.device_id.unwrap_or(format!("{:08X}", rand::random::<u32>()));
    let access_token = db.create_access_token(&user_id.localpart(), &device_id).await?;
    let access_token = format!("{}", access_token.to_hyphenated());

    Ok(Json(json!({
        "user_id": user_id,
        "access_token": access_token,
        "device_id": device_id
    })))
}
