extern crate tokio_postgres as pg;

use actix_web::{App, middleware::Logger, web::{self, JsonConfig}};
use serde::Deserialize;
use std::{
    fmt::{Debug, Result as FmtResult, Formatter},
    sync::Arc,
};

mod client;
mod events;
mod log;
mod storage;
mod util;

#[derive(Deserialize)]
pub struct Config {
    domain: String,
    bind_address: String,
}

pub struct ServerState {
    pub config: Config,
    pub db_pool: storage::postgres::DbPool,
}

impl Debug for ServerState {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "ServerState {{ ... }}")
    }
}

fn main() {
    run().map_err(|e| {
        eprintln!("{:?}", e);
    }).unwrap();
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = toml::from_slice(&std::fs::read("config.toml")?)?;
    let db_pool = storage::postgres::DbPool::new(String::from("host=/run/postgresql/ user=postgres dbname=matrix"), 64);
    let server_state = Arc::new(ServerState { config, db_pool });

    let server_state2 = Arc::clone(&server_state);
    actix_web::HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .data(Arc::clone(&server_state))
            .data(JsonConfig::default().error_handler(|e, _req| client::error::Error::from(e).into()))
            .service(web::scope("/_matrix/client").configure(client::cs_api))
    })
    .bind(&server_state2.config.bind_address)?
    .run()?;

    Ok(())
}
