#![feature(type_ascription)]

extern crate tokio_postgres as pg;

use futures::compat::Future01CompatExt;
use serde::Deserialize;
use std::{
    fmt::{Debug, Result as FmtResult, Formatter},
    sync::Arc,
};

mod client;
mod db_pool;
mod events;

use db_pool::DbPool;

#[derive(Deserialize)]
pub struct Config {
    domain: String,
}

pub struct ServerState {
    pub config: Config,
    pub db_pool: DbPool,
}

impl Debug for ServerState {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "ServerState {{ ... }}")
    }
}

#[runtime::main(runtime_tokio::Tokio)]
async fn main() {
    run().await.map_err(|e| {
        eprintln!("{:?}", e);
    }).unwrap();
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing::subscriber::set_global_default(tracing_fmt::FmtSubscriber::builder().finish()).unwrap();
    tracing_log::LogTracer::init()?;

    let config = toml::from_slice(&tokio::fs::read("./config.toml").compat().await?)?;
    let db_pool = DbPool::new(String::from("host=/run/postgresql/ user=postgres dbname=matrix"), 64);
    let server_state = Arc::new(ServerState { config, db_pool });

    let client_app = client::client_app(server_state);
    client_app.serve("0.0.0.0:8080").await?;

    Ok(())
}
