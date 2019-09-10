use serde_json::json;
use std::sync::Arc;
use tide::{
    cors::CorsMiddleware,
    http::header::HeaderValue,
    middleware::RequestLogger,
    response::{self, Response},
    Context,
};

use crate::ServerState;

mod auth;
mod error;
mod sync;
mod user;

pub type ClientResult = Result<Response, error::Error>;

pub fn client_app(state: Arc<ServerState>) -> tide::App<Arc<ServerState>> {
    let mut app = tide::App::with_state(state);
    /*app.middleware(
        CorsMiddleware::new()
            .allow_origin("*")
            .allow_methods(HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"))
            .allow_headers(HeaderValue::from_static(
                "Origin, X-Requested-With, Content-Type, Accept, Authorization",
            )),
        );*/
    app.middleware(RequestLogger::new());
    let mut client_api = app.at("/_matrix/client");
    client_api.at("/versions").get(versions);
    let mut r0 = client_api.at("r0");
    r0.at("/login")
        .get(auth::get_supported_login_types)
        .post(auth::login);
    r0.at("/logout")
        .post(auth::logout);
    r0.at("/logout/all")
        .post(auth::logout_all);
    r0.at("/profile/:user_id")
        .get(user::get_profile);
    r0.at("/profile/:user_id/avatar_url")
        .get(user::get_avatar_url)
        .put(user::set_avatar_url);
    r0.at("/profile/:user_id/displayname")
        .get(user::get_display_name)
        .put(user::set_display_name);
    app
}

async fn versions(_cx: Context<Arc<ServerState>>) -> ClientResult {
    Ok(response::json(json!({
        "versions": [
            "r0.5.0"
        ]
    })))
}
