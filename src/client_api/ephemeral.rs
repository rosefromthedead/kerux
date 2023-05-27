use actix_web::{
    put,
    web::{Data, Json, Path},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{field::Empty, instrument, Level, Span};

use crate::{
    client_api::auth::AccessToken,
    error::{Error, ErrorKind},
    util::MatrixId,
    ServerState,
};

#[derive(Deserialize)]
pub struct TypingRequest {
    typing: bool,
    #[serde(default)]
    timeout: u32,
}

#[put("/rooms/{room_id}/typing/{user_id}")]
#[instrument(skip(state, token, req), fields(username = Empty), err = Level::DEBUG)]
pub async fn typing(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    Path((room_id, user_id)): Path<(String, MatrixId)>,
    req: Json<TypingRequest>,
) -> Result<Json<Value>, Error> {
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?.ok_or(ErrorKind::Forbidden)?;
    Span::current().record("username", &username.as_str());

    if (username.as_str(), state.config.domain.as_str()) != (user_id.localpart(), user_id.domain())
    {
        return Err(ErrorKind::Forbidden.into());
    }
    db.set_typing(&room_id, &user_id, req.typing, req.timeout)
        .await?;
    Ok(Json(json!({})))
}
