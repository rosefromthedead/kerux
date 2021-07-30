use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{events::EventContent, util::MatrixId, validate::auth::AuthStatus};

use super::{Event, room_version::VersionedPdu};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredPdu {
    inner: VersionedPdu,
    auth_status: AuthStatus,
}

impl StoredPdu {
    pub fn did_pass_auth(&self) -> bool {
        self.auth_status == AuthStatus::Pass
    }

    pub fn inner(&self) -> &VersionedPdu {
        &self.inner
    }

    pub fn to_client_format(self) -> Event {
        self.inner.to_client_format()
    }

    pub fn event_content(&self) -> EventContent {
        self.inner.event_content()
    }

    pub fn room_id(&self) -> &str {
        self.inner.room_id()
    }

    pub fn sender(&self) -> &MatrixId {
        self.inner.sender()
    }

    pub fn state_key(&self) -> Option<&str> {
        self.inner.state_key()
    }

    pub fn unsigned(&self) -> Option<&JsonValue> {
        self.inner.unsigned()
    }

    pub fn redacts(&self) -> Option<&str> {
        self.inner.redacts()
    }

    pub fn origin(&self) -> &str {
        self.inner.origin()
    }

    pub fn origin_server_ts(&self) -> i64 {
        self.inner.origin_server_ts()
    }

    pub fn prev_events(&self) -> &[String] {
        self.inner.prev_events()
    }

    pub fn auth_events(&self) -> &[String] {
        self.inner.auth_events()
    }

    pub fn depth(&self) -> i64 {
        self.inner.depth()
    }

    // TODO: actually completely wrong
    // event_id should probably be stored in StoredPdu because it is not part of a pdu
    pub fn event_id(&self) -> &str {
        self.inner.event_id()
    }
}
