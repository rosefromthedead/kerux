use async_trait::async_trait;
use enum_extract::extract;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{HashSet, HashMap};
use uuid::Uuid;

use crate::{error::Error, events::{Event, EventContent, pdu::StoredPdu, room::Membership, room_version::VersionedPdu}, util::MatrixId};

#[cfg(feature = "storage-mem")]
pub mod mem;
#[cfg(feature = "storage-sled")]
pub mod sled;
#[cfg(feature = "storage-postgres")]
pub mod postgres;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    pub fn matches(&self, pdu: &VersionedPdu) -> bool {
        // We don't have access to the event's ordering in storage, so we can't test whether it
        // exists within the bounds specified in Timeline/State. Therefore we just assume it does.
        match self.query_type {
            QueryType::State { state_keys, not_state_keys, .. } => {
                match &pdu.state_key() {
                    Some(k) => {
                        if not_state_keys.contains(k) {
                            return false;
                        }
                        if !state_keys.is_empty() && !state_keys.contains(k) {
                            return false;
                        }
                    },
                    None => return false,
                }
            },
            // See the comment above
            QueryType::Timeline { .. } => {},
        }

        if self.not_senders.contains(&pdu.sender()) {
            return false;
        }
        if !self.senders.is_empty() && !self.senders.contains(&pdu.sender()) {
            return false;
        }

        if self.not_types.contains(&pdu.event_content().get_type()) {
            return false;
        }
        if !self.types.is_empty() && !self.types.contains(&pdu.event_content().get_type()) {
            return false;
        }

        if let Some(ref value) = self.contains_json {
            let map = value.as_object().expect("contains_json must be an object");
            for (key, value) in map.iter() {
                if pdu.event_content().content_as_json().get(key) != Some(value) {
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

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Batch {
    /// Indices into the event storage of the rooms that the user is in.
    pub rooms: HashMap<String, usize>,
    /// A set of rooms to which the user has been invited, where they are already aware of this.
    pub invites: HashSet<String>,
}

#[async_trait]
pub trait StorageManager: Send + Sync {
    async fn get_handle(&self) -> Result<Box<dyn Storage>, Error>;
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn create_user(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(), Error>;

    async fn verify_password(
        &self,
        username: &str,
        password: &str,
    ) -> Result<bool, Error>;

    async fn create_access_token(
        &self,
        username: &str,
        device_id: &str,
    ) -> Result<Uuid, Error>;

    async fn delete_access_token(&self, token: Uuid) -> Result<(), Error>;

    /// Deletes all access tokens associated with the same user as this one
    async fn delete_all_access_tokens(&self, token: Uuid) -> Result<(), Error>;

    /// Returns the username for which this token is valid, if any
    async fn try_auth(&self, token: Uuid) -> Result<Option<String>, Error>;

    /// Records a transaction ID into the given access token and returns whether it is new
    /// (unique).
    async fn record_txn(&self, token: Uuid, txn_id: String) -> Result<bool, Error>;

    /// Returns the given user's avatar URL and display name, if present
    async fn get_profile(&self, username: &str) -> Result<Option<UserProfile>, Error>;

    async fn set_avatar_url(&self, username: &str, avatar_url: &str)
        -> Result<(), Error>;

    async fn set_display_name(
        &self,
        username: &str,
        display_name: &str,
    ) -> Result<(), Error>;

    async fn add_pdus(&self, pdus: &[StoredPdu]) -> Result<(), Error>;

    async fn get_prev_events(&self, room_id: &str) -> Result<(Vec<String>, i64), Error>;

    async fn query_pdus<'a>(
        &self,
        query: EventQuery<'a>,
        wait: bool,
    ) -> Result<(Vec<StoredPdu>, usize), Error>;

    async fn query_events<'a>(
        &self,
        query: EventQuery<'a>,
        wait: bool,
    ) -> Result<(Vec<Event>, usize), Error> {
        let (pdus, next_batch) = self.query_pdus(query, wait).await?;
        return Ok((pdus.into_iter().map(StoredPdu::to_client_format).collect(), next_batch));
    }

    async fn get_rooms(&self) -> Result<Vec<String>, Error>;

    async fn get_membership(
        &self,
        user_id: &MatrixId,
        room_id: &str,
    ) -> Result<Option<Membership>, Error> {
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
        let membership = event.map(
            |e| extract!(EventContent::Member(_), e.event_content).unwrap().membership);
        Ok(membership)
    }

    /// Returns the number of users in a room and the number of users invited to the room.
    ///
    /// Returns (0, 0) if the room does not exist.
    async fn get_room_member_counts(
        &self,
        room_id: &str,
    ) -> Result<(usize, usize), Error> {
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

    async fn get_full_state(&self, room_id: &str) -> Result<Vec<Event>, Error> {
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
    ) -> Result<Option<Event>, Error> {
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

    async fn get_pdu(
        &self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<StoredPdu>, Error>;

    async fn get_all_ephemeral(
        &self,
        room_id: &str,
    ) -> Result<HashMap<String, JsonValue>, Error>;

    async fn get_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> Result<Option<JsonValue>, Error>;

    async fn set_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
        content: Option<JsonValue>,
    ) -> Result<(), Error>;

    async fn set_typing(
        &self,
        room_id: &str,
        user_id: &MatrixId,
        is_typing: bool,
        timeout: u32,
    ) -> Result<(), Error>;

    async fn get_user_account_data(
        &self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Error>;

    async fn get_batch(&self, id: &str) -> Result<Option<Batch>, Error>;

    async fn set_batch(&self, id: &str, batch: Batch) -> Result<(), Error>;

    async fn print_the_world(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Storage, StorageManager};

    #[cfg(feature = "storage-mem")]
    #[test]
    fn mem_backend_user_accounts() {
        let mut rt = tokio::runtime::Builder::new().basic_scheduler().build().unwrap();
        let db_pool = super::mem::MemStorageManager::new();
        rt.block_on(async {
            let db = db_pool.get_handle().await.unwrap();
            user_accounts(&*db).await;
        });
    }

    #[cfg(feature = "storage-sled")]
    #[test]
    fn sled_backend_user_accounts() {
        let path = "sled-test-user-accounts";
        let _ = std::fs::remove_dir_all(path);
        let mut rt = tokio::runtime::Builder::new().basic_scheduler().build().unwrap();
        let db_pool = super::sled::SledStorage::new(path).unwrap();
        rt.block_on(async {
            let db = db_pool.get_handle().await.unwrap();
            user_accounts(&*db).await;
        });
        let _ = std::fs::remove_dir_all(path);
    }

    async fn user_accounts(db: &dyn Storage) {
        db.create_user("alice", "password1").await.expect("failed to create first user");
        db.create_user("alice", "password1").await.expect_err("succeeded making same user twice");
        db.create_user("alice", "password2").await.expect_err("succeeded making same user twice");
        db.create_user("bob", "password1").await.expect("failed to create second user");

        assert!(db.verify_password("alice", "password1").await.unwrap() == true);
        assert!(db.verify_password("alice", "password2").await.unwrap() == false);
        assert!(db.verify_password("bob", "password1").await.unwrap() == true);
        assert!(db.verify_password("bob", "password2").await.unwrap() == false);

        let alice_token_1 = db.create_access_token("alice", "phone").await.expect("failed to create token");
        let alice_token_2 = db.create_access_token("alice", "laptop").await.expect("failed to create token");
        let bob_token_1 = db.create_access_token("bob", "laptop").await.expect("failed to create token");
        let bob_token_2 = db.create_access_token("bob", "phone").await.expect("failed to create token");
        db.create_access_token("nobody", "kjfnkfn").await.expect_err("succeeded making token for nobody");

        db.delete_access_token(alice_token_2).await.expect("failed to delete token");
        db.delete_access_token(bob_token_2).await.expect("failed to delete token");

        assert_eq!(db.try_auth(alice_token_1).await.expect("failed during auth").as_deref(), Some("alice"));
        assert_eq!(db.try_auth(alice_token_2).await.expect("failed during auth").as_deref(), None);
        assert_eq!(db.try_auth(bob_token_1).await.expect("failed during auth").as_deref(), Some("bob"));
        assert_eq!(db.try_auth(bob_token_2).await.expect("failed during auth").as_deref(), None);

        db.delete_all_access_tokens(alice_token_1).await.expect("failed to delete all tokens");
        assert_eq!(db.try_auth(alice_token_1).await.expect("failed during auth").as_deref(), None);
        assert_eq!(db.try_auth(bob_token_1).await.expect("failed during auth").as_deref(), Some("bob"));
    }

    #[cfg(feature = "storage-mem")]
    #[test]
    fn mem_backend_transactions() {
        let mut rt = tokio::runtime::Builder::new().basic_scheduler().build().unwrap();
        let db_pool = super::mem::MemStorageManager::new();
        rt.block_on(async {
            let db = db_pool.get_handle().await.unwrap();
            transactions(&*db).await;
        });
    }

    #[cfg(feature = "storage-sled")]
    #[test]
    fn sled_backend_transactions() {
        let path = "sled-test-transactions";
        let _ = std::fs::remove_dir_all(path);
        let mut rt = tokio::runtime::Builder::new().basic_scheduler().build().unwrap();
        let db_pool = super::sled::SledStorage::new(path).unwrap();
        rt.block_on(async {
            let db = db_pool.get_handle().await.unwrap();
            transactions(&*db).await;
        });
        let _ = std::fs::remove_dir_all(path);
    }

    async fn transactions(db: &dyn Storage) {
        db.create_user("alice", "password").await.unwrap();
        let token = db.create_access_token("alice", "phone").await.unwrap();
        assert_eq!(db.record_txn(token, String::from("txn1")).await.expect("failed to record transaction"), true);
        assert_eq!(db.record_txn(token, String::from("txn1")).await.expect("failed to record transaction"), false);
        assert_eq!(db.record_txn(token, String::from("txn2")).await.expect("failed to record transaction"), true);
    }
}
