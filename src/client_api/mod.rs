use actix_web::{
    get,
    web::{self, Json},
};
use serde_json::json;

mod auth;
mod ephemeral;
mod room;
mod room_events;
mod user;

pub fn configure_endpoints(cfg: &mut web::ServiceConfig) {
    cfg.service(versions);
    let r0 = web::scope("/r0")
        .service(auth::get_supported_login_types)
        .service(auth::login)
        .service(auth::logout)
        .service(auth::logout_all)
        .service(auth::register)
        .service(user::get_avatar_url)
        .service(user::set_avatar_url)
        .service(user::get_display_name)
        .service(user::set_display_name)
        .service(user::get_profile)
        .service(user::search_user_directory)
        .service(user::get_3pids)
        .service(room::create_room)
        .service(room::invite)
        .service(room::join_by_id_or_alias)
        .service(room_events::sync)
        .service(room_events::get_event)
        .service(room_events::get_state_event_no_key)
        .service(room_events::get_state_event_key)
        .service(room_events::get_state)
        .service(room_events::get_members)
        .service(room_events::send_state_event)
        .service(room_events::send_event)
        .service(ephemeral::typing)
        .wrap(
            actix_cors::Cors::default()
                .send_wildcard()
                .allowed_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
                .allowed_headers(vec![
                    "Origin",
                    "X-Requested-With",
                    "Content-Type",
                    "Accept",
                    "Authorization",
                ]),
        );

    cfg.service(r0);
}

#[get("/versions")]
async fn versions() -> Json<serde_json::Value> {
    Json(json!({
        "versions": [
            "r0.5.0",
            "r0.6.0"
        ]
    }))
}
