use async_trait::async_trait;
use displaydoc::Display;
use log::info;
use serde_json::Value as JsonValue;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc, time::{Duration, Instant},
};
use tokio::sync::{RwLock, broadcast::{channel, Sender}};
use uuid::Uuid;

use crate::{
    client::error::Error as ClientApiError,
    events::{ephemeral::Typing, room::Membership, Event, EventContent, PduV4, UnhashedPdu},
    storage::{EventQuery, QueryType, UserProfile},
    util::MatrixId,
};
use super::Batch;

struct MemStorage {
    rooms: HashMap<String, Room>,
    users: Vec<User>,
    access_tokens: HashMap<Uuid, String>,
    batches: HashMap<String, Batch>,
    txn_ids: HashMap<Uuid, HashSet<String>>,
}

#[derive(Debug)]
struct Room {
    events: Vec<PduV4>,
    ephemeral: HashMap<String, JsonValue>,
    typing: HashMap<MatrixId, Instant>,
    notify_send: Sender<()>,
}

#[derive(Debug)]
struct User {
    username: String,
    password_hash: String,
    profile: UserProfile,
    account_data: HashMap<String, JsonValue>,
}

pub struct MemStorageManager {
    storage: Arc<RwLock<MemStorage>>,
}

pub struct MemStorageHandle {
    inner: Arc<RwLock<MemStorage>>,
}

#[derive(Debug, Display)]
pub enum Error {
    /// The specified user was not found.
    UserNotFound,
    /// The specified room was not found.
    RoomNotFound,
    /// The specified access token was not found.
    AccessTokenNotFound,
}

impl Room {
    fn new() -> Self {
        Room {
            events: Vec::new(),
            ephemeral: HashMap::new(),
            typing: Default::default(),
            notify_send: channel(1).0,
        }
    }
}

impl std::error::Error for Error {}

impl MemStorageManager {
    pub fn new() -> Self {
        MemStorageManager {
            storage: Arc::new(RwLock::new(MemStorage {
                rooms: HashMap::new(),
                users: Vec::new(),
                access_tokens: HashMap::new(),
                batches: HashMap::new(),
                txn_ids: HashMap::new(),
            })),
        }
    }
}

#[async_trait]
impl super::StorageManager for MemStorageManager {
    type Handle = MemStorageHandle;
    type Error = Error;

    async fn get_handle(&self) -> Result<Self::Handle, Self::Error> {
        Ok(MemStorageHandle {
            inner: Arc::clone(&self.storage),
        })
    }
}

#[async_trait]
impl super::Storage for MemStorageHandle {
    type Error = Error;

    async fn create_user(
        &self,
        username: &str,
        password_hash: Option<&str>,
    ) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.users.push(User {
            username: username.to_string(),
            password_hash: password_hash.unwrap().to_string(),
            profile: UserProfile {
                avatar_url: None,
                displayname: None,
            },
            account_data: HashMap::new(),
        });
        Ok(())
    }

    async fn verify_password(&self, username: &str, password: &str) -> Result<bool, Error> {
        let db = self.inner.read().await;
        let user = db.users.iter().find(|u| u.username == username);
        if let Some(user) = user {
            match argon2::verify_encoded(&user.password_hash, password.as_bytes()) {
                Ok(true) => Ok(true),
                Ok(false) => Ok(false),
                Err(e) => {
                    log::warn!("password error: {}", e);
                    Ok(false)
                }
            }
        } else {
            Ok(false)
        }
    }

    async fn create_access_token(
        &self,
        username: &str,
        _device_id: &str,
    ) -> Result<Uuid, Error> {
        let mut db = self.inner.write().await;
        let token = Uuid::new_v4();
        db.access_tokens.insert(token, username.to_string());
        Ok(token)
    }

    async fn delete_access_token(&self, token: Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.access_tokens.remove(&token);
        Ok(())
    }

    async fn delete_all_access_tokens(&self, token: Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let username = match db.access_tokens.get(&token) {
            Some(v) => v.clone(),
            None => return Ok(()),
        };
        db.access_tokens.retain(|_token, name| *name != username);
        Ok(())
    }

    async fn try_auth(&self, token: Uuid) -> Result<Option<String>, Error> {
        let db = self.inner.read().await;
        Ok(db.access_tokens.get(&token).cloned())
    }

    async fn record_txn(&self, token: Uuid, txn_id: String) -> Result<bool, Self::Error> {
        let mut db = self.inner.write().await;
        let set = db.txn_ids.entry(token).or_insert_with(HashSet::new);
        Ok(set.insert(txn_id))
    }

    async fn get_profile(&self, username: &str) -> Result<Option<UserProfile>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.profile.clone()))
    }

    async fn set_avatar_url(&self, username: &str, avatar_url: &str) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let user = db
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or(Error::UserNotFound)?;
        user.profile.avatar_url = Some(avatar_url.to_string());
        Ok(())
    }

    async fn set_display_name(&self, username: &str, display_name: &str) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let user = db
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or(Error::UserNotFound)?;
        user.profile.displayname = Some(display_name.to_string());
        Ok(())
    }

    async fn add_pdus(&self, pdus: &[PduV4]) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        for pdu in pdus {
            match pdu.event_content {
                EventContent::Create(_) => {
                    db.rooms.insert(
                        pdu.room_id.clone(),
                        Room::new(),
                    );
                }
                _ => {},
            }
            db.rooms
                .get_mut(&pdu.room_id)
                .ok_or(Error::RoomNotFound)?
                .events
                .push(pdu.clone());
        }
        Ok(())
    }

    async fn add_event_unchecked(&self, event: Event, auth_events: Vec<String>) -> Result<String, Error> {
        let mut db = self.inner.write().await;
        let room_id = event.room_id.as_ref().unwrap();
        let room = db.rooms.get_mut(room_id).ok_or(Error::RoomNotFound)?;
        let depth = room.events.iter().map(|pdu| pdu.depth).max().unwrap();
        let prev_events = room.events.iter()
            .filter(|pdu| pdu.depth == depth)
            .map(|pdu| pdu.event_id())
            .collect::<Vec<_>>();
        let Event {
            event_content,
            room_id,
            sender,
            state_key,
            unsigned,
            redacts,
            event_id: _,
            origin_server_ts: _,
        } = event;
        let origin = String::from(sender.domain());
        let origin_server_ts = chrono::Utc::now().timestamp_millis();
        let pdu = UnhashedPdu {
            event_content,
            room_id: room_id.unwrap(),
            sender,
            state_key,
            unsigned,
            redacts,
            origin: origin.clone(),
            origin_server_ts,
            prev_events,
            depth: depth + 1,
            auth_events,
        }.finalize();
        let hash = pdu.hashes.sha256.clone();
        let event_id = pdu.event_id();
        tracing::trace!(?pdu, "Adding event to storage");
        room.events.push(pdu);
        let _ = room.notify_send.send(());
        Ok(event_id)
    }

    async fn query_pdus<'a>(
        &self,
        query: EventQuery<'a>,
        wait: bool,
    ) -> Result<(Vec<PduV4>, usize), Error> {
        let mut ret = Vec::new();
        let (mut from, mut to) = match &query.query_type {
            &QueryType::Timeline { from, to } => {
                (from, to)
            },
            &QueryType::State { at, .. } => {
                (0, at)
            },
        };

        let db = self.inner.read().await;
        let room = db.rooms.get(query.room_id).ok_or(Error::RoomNotFound)?;
        if let None = to {
            to = Some(room.events.len() - 1);
        }

        if let Some(range) = room.events.get(from..=to.unwrap()) {
            ret.extend(
                range.iter()
                .filter(|pdu| query.matches(&pdu))
                .cloned());
        }

        if wait && ret.is_empty() && query.query_type.is_timeline() {
            let mut recv = room.notify_send.subscribe();
            // Release locks; we are about to wait for new events to come in, and they can't if we've
            // locked the db
            drop(db);
            // This returns a result, but one of the possible errors is "there are multiple
            // events" which is what we're waiting for anyway, and the other is "send half has
            // been dropped" which would mean we have bigger problems than this one query
            let _ = recv.recv().await;
            from = to.unwrap();
            to = None;
        } else {
            return Ok((ret, to.unwrap()));
        }

        // same again
        let db = self.inner.read().await;
        let room = db.rooms.get(query.room_id).ok_or(Error::RoomNotFound)?;
        if let None = to {
            to = Some(room.events.len() - 1);
        }

        if let Some(range) = room.events.get(from..=to.unwrap()) {
            ret.extend(
                range.iter()
                .filter(|pdu| query.matches(&pdu))
                .cloned());
        }
        Ok((ret, to.unwrap()))
    }

    async fn get_memberships_by_user(
        &self,
        user_id: &MatrixId,
    ) -> Result<HashMap<String, Membership>, Error> {
        let rooms = {
            let db = self.inner.read().await;
            db.rooms.keys().cloned().collect::<Vec<_>>()
        };
        let mut ret = HashMap::new();
        for room_id in rooms {
            let membership = self.get_membership(user_id, &room_id).await?;
            if let Some(membership) = membership {
                ret.insert(room_id.to_string(), membership);
            }
        }
        Ok(ret)
    }

    async fn get_event(
        &self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<Event>, Error> {
        let db = self.inner.read().await;
        let event = db
            .rooms
            .get(room_id)
            .map(|r| r.events.iter().find(|e| e.hashes.sha256 == event_id))
            .flatten()
            .map(|event| Event {
                event_content: event.event_content.clone(),
                room_id: None,
                sender: event.sender.clone(),
                state_key: event.state_key.clone(),
                unsigned: event.unsigned.clone(),
                redacts: event.redacts.clone(),
                event_id: Some(event.hashes.sha256.clone()),
                origin_server_ts: Some(event.origin_server_ts),
            });
        Ok(event)
    }

    async fn get_all_ephemeral(
        &self,
        room_id: &str,
    ) -> Result<HashMap<String, JsonValue>, Self::Error> {
        let db = self.inner.read().await;
        let room = db.rooms.get(room_id).ok_or(Error::RoomNotFound)?;
        let mut ephemeral = room.ephemeral.clone();

        let now = Instant::now();
        let mut typing = Typing::default();
        for (mxid, _) in room.typing.iter().filter(|(_, timeout)| **timeout < now) {
            typing.user_ids.insert(mxid.clone());
        }
        ephemeral.insert(String::from("m.typing"), serde_json::to_value(typing).unwrap());
        Ok(ephemeral)
    }

    async fn get_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> Result<Option<JsonValue>, Error> {
        let db = self.inner.read().await;
        let room = db.rooms.get(room_id).ok_or(Error::RoomNotFound)?;
        if event_type == "m.typing" {
            let now = Instant::now();
            let mut ret = Typing::default();
            for (mxid, _) in room.typing.iter().filter(|(_, timeout)| **timeout > now) {
                ret.user_ids.insert(mxid.clone());
            }
            return Ok(Some(serde_json::to_value(ret).unwrap()))
        }
        Ok(room.ephemeral.get(event_type).cloned())
    }

    async fn set_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
        content: Option<JsonValue>,
    ) -> Result<(), Self::Error> {
        assert!(event_type != "m.typing", "m.typing should not be set directly");
        let mut db = self.inner.write().await;
        let room = db.rooms.get_mut(room_id).ok_or(Error::RoomNotFound)?;
        match content {
            Some(c) => room.ephemeral.insert(String::from(event_type), c),
            None => room.ephemeral.remove(event_type),
        };
        let _ = room.notify_send.send(());
        Ok(())
    }

    async fn set_typing(
        &self,
        room_id: &str,
        user_id: &MatrixId,
        is_typing: bool,
        timeout: u32,
    ) -> Result<(), Self::Error> {
        let mut db = self.inner.write().await;
        let room = db.rooms.get_mut(room_id).ok_or(Error::RoomNotFound)?;
        if is_typing {
            room.typing.insert(user_id.clone(), Instant::now() + Duration::from_millis(timeout as u64));
        } else {
            room.typing.remove(user_id);
        }
        let _ = room.notify_send.send(());

        Ok(())
    }

    async fn get_user_account_data(
        &self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Error> {
        let db = self.inner.read().await;
        let map = db
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.account_data.clone())
            .unwrap_or(HashMap::new());
        Ok(map)
    }

    async fn get_batch(&self, id: &str) -> Result<Option<Batch>, Error> {
        let db = self.inner.read().await;
        Ok(db.batches.get(id).cloned())
    }

    async fn set_batch(&self, id: &str, batch: Batch) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let _ = db.batches.insert(String::from(id), batch);
        Ok(())
    }

    async fn print_the_world(&self) -> Result<(), Error> {
        let db = self.inner.read().await;
        println!("{:#?}", db.rooms);
        println!("{:#?}", db.users);
        println!("{:#?}", db.access_tokens);
        Ok(())
    }
}
