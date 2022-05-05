use actix_web::client::ClientRequest;
use serde_json::{json, Value as JsonValue};

use crate::sign::sign_json;

pub fn server_request(
    mut req: ClientRequest,
    state: &crate::ServerState,
    body: Option<&JsonValue>,
) -> Result<ClientRequest, serde_canonical::error::Error> {
    let uri = req.get_uri();
    let mut req_object = json!({
        "method": req.get_method().as_str(),
        "uri": uri.path_and_query().unwrap().as_str(),
        "origin": state.config.domain,
        "destination": uri.host().unwrap(),
    });
    if let Some(body) = body {
        req_object
            .as_object_mut()
            .unwrap()
            .insert(String::from("content"), body.clone());
    }

    let signatures = sign_json(&req_object, &state.keys)?;

    for (key, sig) in signatures.iter() {
        req = req.header(
            "X-Matrix",
            format!("origin={},key={key},sig={sig}", state.config.domain),
        );
    }

    Ok(req)
}
