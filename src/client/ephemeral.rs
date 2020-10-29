use actix_web::{
    put,
    web::{Data, Json, Path},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    client::{auth::AccessToken, error::Error},
    storage::{Storage, StorageManager},
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
pub async fn typing(
    state: Data<Arc<ServerState>>,
    token: AccessToken,
    path_args: Path<(String, MatrixId)>,
    req: Json<TypingRequest>,
) -> Result<Json<()>, Error> {
    let (room_id, user_id) = path_args.into_inner();
    let db = state.db_pool.get_handle().await?;
    let username = db.try_auth(token.0).await?;
    if (username.as_deref(), state.config.domain.as_str()) != (Some(user_id.localpart()), user_id.domain()) {
        return Err(Error::Forbidden);
    }
    db.set_typing(&room_id, &user_id, req.typing, req.timeout).await?;
    Ok(Json(()))
}
