use actix_web::{post, web::Data};
use std::sync::Arc;

use crate::{storage::{Storage, StorageManager}, ServerState};

pub mod mxid;
pub mod storage;

pub use storage::StorageExt;
pub use mxid::{MatrixId, MxidError};

#[post("/_debug/print_the_world")]
pub async fn print_the_world(state: Data<Arc<ServerState>>) -> String {
    let mut db = state.db_pool.get_handle().await.unwrap();
    db.print_the_world().await.unwrap();
    String::new()
}
