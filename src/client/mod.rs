use actix_web::{get, web::{self, Json}};
use serde_json::json;

mod auth;
pub mod error;
/*mod room;
mod sync;
mod user;*/

pub fn cs_api(cfg: &mut web::ServiceConfig) {
    /*app.middleware(
    CorsMiddleware::new()
        .allow_origin("*")
        .allow_methods(HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"))
        .allow_headers(HeaderValue::from_static(
            "Origin, X-Requested-With, Content-Type, Accept, Authorization",
        )),
    );*/
    //    app.middleware(crate::log::handle);
    cfg.service(versions);
    let r0 = web::scope("/r0")
        .service(auth::get_supported_login_types)
        .service(auth::login)
        .service(auth::logout)
        .service(auth::logout_all)
        .service(auth::register);
    /*    r0.at("/profile/:user_id")
        .get(user::get_profile);
    r0.at("/profile/:user_id/avatar_url")
        .get(user::get_avatar_url)
        .put(user::set_avatar_url);
    r0.at("/profile/:user_id/displayname")
        .get(user::get_display_name)
        .put(user::set_display_name);

    r0.at("/createRoom")
        .post(room::create_room);*/

    cfg.service(r0);
}

#[get("/versions")]
async fn versions() -> Json<serde_json::Value> {
    Json(json!({
        "versions": [
            "r0.6.0"
        ]
    }))
}
