extern crate tokio_postgres as pg;

use actix_web::{App, web::{self, JsonConfig}};
use serde::Deserialize;
use std::{
    fmt::{Debug, Result as FmtResult, Formatter},
    sync::Arc,
};

mod client;
mod events;
mod log;
mod storage;

#[derive(Deserialize)]
pub struct Config {
    domain: String,
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
    tracing::subscriber::set_global_default(tracing_fmt::FmtSubscriber::builder().finish())?;
    tracing_log::LogTracer::init()?;

    let config = toml::from_slice(&std::fs::read("config.toml")?)?;
    let db_pool = storage::postgres::DbPool::new(String::from("host=/run/postgresql/ user=postgres dbname=matrix"), 64);
    let server_state = Arc::new(ServerState { config, db_pool });

    actix_web::HttpServer::new(move || {
        App::new()
            .data(Arc::clone(&server_state))
            .data(JsonConfig::default().error_handler(|e, _req| client::error::Error::from(e).into()))
            .service(web::scope("/_matrix/client").configure(client::cs_api))
    })
    .bind("0.0.0.0:8080")?
    .run()?;

    Ok(())
}
