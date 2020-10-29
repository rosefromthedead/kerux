use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::collections::{HashSet, HashMap};
use uuid::Uuid;

use crate::{
    events::{
        ephemeral::Typing,
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
pub struct EventQuery<'a> {
    pub query_type: QueryType<'a>,
    pub room_id: &'a str,
    /// A list of event senders to include in the result. If the list is empty all senders are
    /// included.
    pub senders: &'a [&'a MatrixId],
    /// A list of event senders to exclude in the result. If the list is empty no senders are
    /// excluded. Exclusion takes priority; if a sender is listed in both `senders` and
    /// `not_senders`, the net result is exclusion.
    pub not_senders: &'a [&'a MatrixId],
    /// A list of event types to include in the result. If the list is empty all types are
    /// included.
    pub types: &'a [&'a str],
    /// A list of event types to exclude in the result. If the list is empty no types are excluded.
    /// Exclusion takes priority; if a type is listed in both `types` and `not_types`, the net
    /// result is exclusion.
    pub not_types: &'a [&'a str],
    /// Only return results whose content fields have identical values to those in here.
    pub contains_json: Option<JsonValue>,
}

#[derive(Clone)]
pub enum QueryType<'a> {
    /// Timeline queries return all events (confusingly, even state events) in a given timeframe.
    Timeline { from: usize, to: Option<usize> },
    /// State queries return all of the most recent state events with unique (type, state_key)
    /// pairs, from a given point in time. This represents the full state of the room at that time.
    State {
        at: Option<usize>,
        /// A list of state keys to include in the result. If the list is empty all keys are
        /// included.
        state_keys: &'a [&'a str],
        /// A list of state keys to exclude in the result. If the list is empty no keys are excluded.
        /// Exclusion takes priority; if a key is listed in both `keys` and `not_keys`, the net
        /// result is exclusion.
        not_state_keys: &'a [&'a str],
    },
}

impl<'a> EventQuery<'a> {
    pub fn matches(&self, pdu: &PduV4) -> bool {
        // We don't have access to the event's ordering in storage, so we can't test whether it
        // exists within the bounds specified in Timeline/State. Therefore we just assume it does.
        match self.query_type {
            QueryType::State { state_keys, not_state_keys, .. } => {
                match &pdu.state_key {
                    Some(k) => {
                        if not_state_keys.contains(&k.as_str()) {
                            return false;
                        }
                        if !state_keys.is_empty() && !state_keys.contains(&k.as_str()) {
                            return false;
                        }
                    },
                    None => return false,
                }
            },
            // See the comment above
            QueryType::Timeline { .. } => {},
        }

        if self.not_senders.contains(&&pdu.sender) {
            return false;
        }
        if !self.senders.is_empty() && !self.senders.contains(&&pdu.sender) {
            return false;
        }

        if self.not_types.contains(&pdu.ty.as_str()) {
            return false;
        }
        if !self.types.is_empty() && !self.types.contains(&pdu.ty.as_str()) {
            return false;
        }

        if let Some(ref value) = self.contains_json {
            let map = value.as_object().expect("contains_json must be an object");
            for (key, value) in map.iter() {
                if pdu.content.get(key) != Some(value) {
                    return false;
                }
            }
        }

        true
    }
}

impl<'a> QueryType<'a> {
    pub fn is_timeline(&self) -> bool {
        match self {
            QueryType::Timeline { .. } => true,
            _ => false,
        }
    }

    pub fn is_state(&self) -> bool {
        match self {
            QueryType::State { .. } => true,
            _ => false,
        }
    }
}

#[derive(Clone, Default)]
pub struct Batch {
    /// Indices into the event storage of the rooms that the user is in.
    pub rooms: HashMap<String, usize>,
    /// A set of rooms to which the user has been invited, where they are already aware of this.
    pub invites: HashSet<String>,
}

#[async_trait]
pub trait StorageManager {
    type Handle: Storage;
    type Error: std::error::Error;

    async fn get_handle(&self) -> Result<Self::Handle, Self::Error>;
}

#[async_trait]
pub trait Storage: Send + Sync {
    type Error: std::error::Error;

    async fn create_user(
        &self,
        username: &str,
        password_hash: Option<&str>, //TODO: accept "password" and do the hashing here
    ) -> Result<(), Self::Error>;

    async fn verify_password(
        &self,
        username: &str,
        password: &str,
    ) -> Result<bool, Self::Error>;

    async fn create_access_token(
        &self,
        username: &str,
        device_id: &str,
    ) -> Result<Uuid, Self::Error>;

    async fn delete_access_token(&self, token: Uuid) -> Result<(), Self::Error>;

    /// Deletes all access tokens associated with the same user as this one
    async fn delete_all_access_tokens(&self, token: Uuid) -> Result<(), Self::Error>;

    /// Returns the username for which this token is valid, if any
    async fn try_auth(&self, token: Uuid) -> Result<Option<String>, Self::Error>;

    /// Returns the given user's avatar URL and display name, if present
    async fn get_profile(&self, username: &str) -> Result<Option<UserProfile>, Self::Error>;

    async fn set_avatar_url(&self, username: &str, avatar_url: &str)
        -> Result<(), Self::Error>;

    async fn set_display_name(
        &self,
        username: &str,
        display_name: &str,
    ) -> Result<(), Self::Error>;

    async fn add_pdus(&self, pdus: &[PduV4]) -> Result<(), Self::Error>;

    /// Adds the given event to the head of the room, *without* checking any of the following:
    /// * whether the event's contents are valid
    /// * whether the sender is allowed to send this event in this context
    /// * whether the given auth_events do actually authorise the event
    /// * whether the given auth_events exist
    ///
    /// This means they **must** be checked before calling. Typically, this is done by
    /// `util::storage::StorageExt::add_event`.
    async fn add_event_unchecked(
        &self,
        event: Event,
        auth_events: Vec<String>,
    ) -> Result<String, Self::Error>;

    async fn query_pdus<'a>(
        &self,
        query: EventQuery<'a>,
        wait: bool,
    ) -> Result<(Vec<PduV4>, usize), Self::Error>;

    async fn query_events<'a>(
        &self,
        query: EventQuery<'a>,
        wait: bool,
    ) -> Result<(Vec<Event>, usize), Self::Error> {
        let (pdus, next_batch) = self.query_pdus(query, wait).await?;
        return Ok((pdus.into_iter().map(PduV4::to_client_format).collect(), next_batch));
    }

    async fn get_memberships_by_user(
        &self,
        user_id: &MatrixId,
    ) -> Result<HashMap<String, Membership>, Self::Error>;

    async fn get_membership(
        &self,
        user_id: &MatrixId,
        room_id: &str,
    ) -> Result<Option<Membership>, Self::Error> {
        let event = self
            .query_events(EventQuery {
                query_type: QueryType::State {
                    at: None,
                    state_keys: &[user_id.as_str()],
                    not_state_keys: &[],
                },
                room_id,
                senders: &[],
                not_senders: &[],
                types: &["m.room.member"],
                not_types: &[],
                contains_json: None,
            }, false)
            .await?
            .0
            .pop();
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
        &self,
        room_id: &str,
    ) -> Result<(usize, usize), Self::Error> {
        let join_query = EventQuery {
            query_type: QueryType::State {
                at: None,
                state_keys: &[],
                not_state_keys: &[],
            },
            room_id,
            senders: &[],
            not_senders: &[],
            types: &["m.room.member"],
            not_types: &[],
            contains_json: Some(serde_json::json!({
                "membership": "join"
            })),
        };
        let mut invited_query = join_query.clone();
        invited_query.contains_json = Some(serde_json::json!({
            "membership": "invite"
        }));

        let join_count = self.query_events(join_query, false).await?.0.len();
        let invited_count = self.query_events(invited_query, false).await?.0.len();

        Ok((join_count, invited_count))
    }

    async fn get_full_state(&self, room_id: &str) -> Result<Vec<Event>, Self::Error> {
        let (ret, _) = self.query_events(EventQuery {
            query_type: QueryType::State {
                at: None,
                state_keys: &[],
                not_state_keys: &[],
            },
            room_id,
            senders: &[],
            not_senders: &[],
            types: &[],
            not_types: &[],
            contains_json: None,
        }, false).await?;
        Ok(ret)
    }

    async fn get_state_event(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<Option<Event>, Self::Error> {
        let ret = self.query_events(EventQuery {
            query_type: QueryType::State {
                at: None,
                state_keys: &[state_key],
                not_state_keys: &[],
            },
            room_id,
            senders: &[],
            not_senders: &[],
            types: &[event_type],
            not_types: &[],
            contains_json: None,
        }, false).await?.0.pop();
        Ok(ret)
    }

    async fn get_event(
        &self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<Event>, Self::Error>;

    async fn get_all_ephemeral(
        &self,
        room_id: &str,
    ) -> Result<HashMap<String, JsonValue>, Self::Error>;

    async fn get_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> Result<Option<JsonValue>, Self::Error>;

    async fn set_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
        content: Option<JsonValue>,
    ) -> Result<(), Self::Error>;

    async fn set_typing(
        &self,
        room_id: &str,
        user_id: &MatrixId,
        is_typing: bool,
        timeout: u32,
    ) -> Result<(), Self::Error>;

    async fn get_user_account_data(
        &self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Self::Error>;

    async fn get_batch(&self, id: &str) -> Result<Option<Batch>, Self::Error>;

    async fn set_batch(&self, id: &str, batch: Batch) -> Result<(), Self::Error>;

    async fn print_the_world(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
