#[cfg(feature = "storage-postgres")]
extern crate tokio_postgres as pg;

use actix_web::{App, web::{self, JsonConfig}};
use error::Error;
use serde::Deserialize;
use state::StateResolver;
use tracing_subscriber::EnvFilter;
use std::sync::Arc;

mod client;
mod error;
mod events;
mod state;
mod storage;
mod util;
mod validate;

use storage::StorageManager;
use util::StorageExt;

#[derive(Deserialize)]
pub struct Config {
    domain: String,
    bind_address: String,
    storage: String,
}

pub struct ServerState {
    pub config: Config,
    pub db_pool: Box<dyn StorageManager>,
    pub state_resolver: StateResolver,
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    run().await.map_err(|e| {
        eprintln!("Error starting the server: {}", e);
        std::io::Error::from(std::io::ErrorKind::Other)
    })
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let config: Config = toml::from_slice(&std::fs::read("config.toml")?)?;
    let db_pool = match &*config.storage {
        "mem" => {
            let storage = Box::new(storage::mem::MemStorageManager::new()) as Box<dyn StorageManager>;
            storage.get_handle().await?.create_test_users().await?;
            storage
        },
        "sled" => Box::new(storage::sled::SledStorage::new("sled")?) as _,
        _ => panic!("invalid storage type"),
    };
    let state_resolver = StateResolver::new(db_pool.get_handle().await?);
    let server_state = Arc::new(ServerState { config, db_pool, state_resolver });

    let server_state2 = Arc::clone(&server_state);
    actix_web::HttpServer::new(move || {
        App::new()
            .data(Arc::clone(&server_state))
            .data(JsonConfig::default().error_handler(|e, _req| Error::from(e).into()))
            .service(web::scope("/_matrix/client").configure(client::cs_api))
            .service(util::print_the_world)
    })
        .bind(&server_state2.config.bind_address)?
        .run()
        .await?;
    Ok(())
}
