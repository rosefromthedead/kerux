use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    events::{
        room::{Member, Membership},
        Event, PduV4,
    },
    util::MatrixId,
};

pub mod mem;
//pub mod postgres;

#[derive(Clone, Debug, Default)]
pub struct UserProfile {
    pub avatar_url: Option<String>,
    pub displayname: Option<String>,
}

#[derive(Clone)]
pub struct EventQuery {
    pub query_type: QueryType,
    pub room: String,
    /// If no events are immediately available, wait this many milliseconds for an event before
    /// giving up.
    pub timeout_ms: usize,
    /// A list of event senders to include in the result. If the list is empty all senders are
    /// included.
    pub senders: Vec<MatrixId>,
    /// A list of event senders to exclude in the result. If the list is empty no senders are
    /// excluded. Exclusion takes priority; if a sender is listed in both `senders` and
    /// `not_senders`, the net result is exclusion.
    pub not_senders: Vec<MatrixId>,
    /// A list of event types to include in the result. If the list is empty all types are
    /// included.
    pub types: Vec<String>,
    /// A list of event types to exclude in the result. If the list is empty no types are excluded.
    /// Exclusion takes priority; if a type is listed in both `types` and `not_types`, the net
    /// result is exclusion.
    pub not_types: Vec<String>,
    /// Only return results whose content fields have identical values to those in here, for only
    /// the keys which are present in both.
    pub contains_json: Option<JsonValue>,
}

#[derive(Clone)]
pub enum QueryType {
    /// Timeline queries return all events (confusingly, even state events) in a given timeframe.
    Timeline { from: usize, to: Option<usize> },
    /// State queries return all of the most recent state events with unique (type, state_key)
    /// pairs, from a given point in time. This represents the full state of the room at that time.
    State {
        at: Option<usize>,
        /// A list of state keys to include in the result. If the list is empty all keys are
        /// included.
        state_keys: Vec<String>,
        /// A list of state keys to exclude in the result. If the list is empty no keys are excluded.
        /// Exclusion takes priority; if a key is listed in both `keys` and `not_keys`, the net
        /// result is exclusion.
        not_state_keys: Vec<String>,
    },
}

#[async_trait]
pub trait StorageManager {
    type Handle: Storage;
    type Error: std::error::Error;

    async fn get_handle(&self) -> Result<Self::Handle, Self::Error>;
}

#[async_trait]
pub trait Storage: Send {
    type Error: std::error::Error;

    async fn create_user(
        &mut self,
        username: &str,
        password_hash: Option<&str>, //TODO: accept "password" and do the hashing here
    ) -> Result<(), Self::Error>;

    async fn verify_password(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<bool, Self::Error>;

    async fn create_access_token(
        &mut self,
        username: &str,
        device_id: &str,
    ) -> Result<Uuid, Self::Error>;

    async fn delete_access_token(&mut self, token: Uuid) -> Result<(), Self::Error>;

    /// Deletes all access tokens associated with the same user as this one
    async fn delete_all_access_tokens(&mut self, token: Uuid) -> Result<(), Self::Error>;

    /// Returns the username for which this token is valid, if any
    async fn try_auth(&mut self, token: Uuid) -> Result<Option<String>, Self::Error>;

    /// Returns the given user's avatar URL and display name, if present
    async fn get_profile(&mut self, username: &str) -> Result<Option<UserProfile>, Self::Error>;

    async fn set_avatar_url(&mut self, username: &str, avatar_url: &str)
        -> Result<(), Self::Error>;

    async fn set_display_name(
        &mut self,
        username: &str,
        display_name: &str,
    ) -> Result<(), Self::Error>;

    async fn add_pdus(&mut self, pdus: &[PduV4]) -> Result<(), Self::Error>;

    async fn query_pdus(&mut self, query: EventQuery) -> Result<Option<Vec<PduV4>>, Self::Error>;

    async fn query_events(
        &mut self,
        query: EventQuery,
    ) -> Result<Option<Vec<Event>>, Self::Error> {
        let pdus = self.query_pdus(query).await?;
        match pdus {
            Some(pdus) => return Ok(Some(pdus.into_iter().map(PduV4::to_client_format).collect())),
            None => return Ok(None),
        }
    }

    async fn get_memberships_by_user(
        &mut self,
        user_id: &MatrixId,
    ) -> Result<HashMap<String, Membership>, Self::Error>;

    async fn get_membership(
        &mut self,
        user_id: &MatrixId,
        room_id: &str,
    ) -> Result<Option<Membership>, Self::Error> {
        let event = self
            .query_events(EventQuery {
                query_type: QueryType::State {
                    at: None,
                    state_keys: vec![user_id.clone_inner()],
                    not_state_keys: Vec::new(),
                },
                room: String::from(room_id),
                timeout_ms: 0,
                senders: Vec::new(),
                not_senders: Vec::new(),
                types: vec![String::from("m.room.member")],
                not_types: Vec::new(),
                contains_json: None,
            })
            .await?
            .map(|mut v| v.pop())
            .flatten();
        let membership = event
            .map(|e| serde_json::from_value(e.content).ok())
            .flatten()
            .map(|c: Member| c.membership);
        Ok(membership)
    }

    /// Returns the number of users in a room and the number of users invited to the room.
    ///
    /// Returns (0, 0) if the room does not exist.
    async fn get_room_member_counts(
        &mut self,
        room_id: &str,
    ) -> Result<(usize, usize), Self::Error> {
        let join_query = EventQuery {
            query_type: QueryType::State {
                at: None,
                state_keys: Vec::new(),
                not_state_keys: Vec::new(),
            },
            room: String::from(room_id),
            timeout_ms: 0,
            senders: Vec::new(),
            not_senders: Vec::new(),
            types: vec![String::from("m.room.member")],
            not_types: Vec::new(),
            contains_json: Some(serde_json::json!({
                "membership": "join"
            })),
        };
        let mut invited_query = join_query.clone();
        invited_query.contains_json = Some(serde_json::json!({
            "membership": "invite"
        }));

        let join_count = self.query_events(join_query).await?.map(|v| v.len());
        let invited_count = self.query_events(invited_query).await?.map(|v| v.len());

        let ret = match (join_count, invited_count) {
            (Some(j), Some(i)) => (j, i),
            _ => (0, 0),
        };
        Ok(ret)
    }

    async fn get_full_state(&mut self, room_id: &str) -> Result<Option<Vec<Event>>, Self::Error> {
        let ret = self.query_events(EventQuery {
            query_type: QueryType::State {
                at: None,
                state_keys: Vec::new(),
                not_state_keys: Vec::new(),
            },
            room: String::from(room_id),
            timeout_ms: 0,
            senders: Vec::new(),
            not_senders: Vec::new(),
            types: Vec::new(),
            not_types: Vec::new(),
            contains_json: None,
        }).await?;
        Ok(ret)
    }

    /// Returns None if the room does not exist, or if it does not have an event of this type and
    /// state key.
    async fn get_state_event(
        &mut self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<Option<Event>, Self::Error> {
        let ret = self.query_events(EventQuery {
            query_type: QueryType::State {
                at: None,
                state_keys: vec![String::from(state_key)],
                not_state_keys: Vec::new(),
            },
            room: String::from(room_id),
            timeout_ms: 0,
            senders: Vec::new(),
            not_senders: Vec::new(),
            types: vec![String::from(event_type)],
            not_types: Vec::new(),
            contains_json: None,
        }).await?.map(|mut v| v.pop()).flatten();
        Ok(ret)
    }

    async fn get_events_since(
        &mut self,
        room_id: &str,
        since: usize,
    ) -> Result<Option<Vec<Event>>, Self::Error> {
        let ret = self.query_events(EventQuery {
            query_type: QueryType::Timeline {
                from: since, to: None,
            },
            room: String::from(room_id),
            timeout_ms: 0,
            senders: Vec::new(),
            not_senders: Vec::new(),
            types: Vec::new(),
            not_types: Vec::new(),
            contains_json: None,
        }).await?;
        Ok(ret)
    }

    async fn get_event(
        &mut self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<Event>, Self::Error>;

    async fn get_prev_event_ids(
        &mut self,
        room_id: &str,
    ) -> Result<Option<(i64, Vec<String>)>, Self::Error>;

    async fn get_user_account_data(
        &mut self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Self::Error>;

    async fn print_the_world(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
