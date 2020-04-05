use async_trait::async_trait;
use displaydoc::Display;
use serde_json::Value as JsonValue;
use std::{
    collections::HashMap,
    sync::Arc,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    events::{room::Membership, Event, PduV4},
    storage::{EventQuery, QueryType, UserProfile},
    util::MatrixId,
};

#[derive(Debug)]
struct MemStorage {
    rooms: HashMap<String, Room>,
    users: Vec<User>,
    access_tokens: HashMap<Uuid, String>,
}

#[derive(Debug)]
struct Room {
    events: Vec<PduV4>,
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
}

impl std::error::Error for Error {}

impl MemStorageManager {
    pub fn new() -> Self {
        MemStorageManager {
            storage: Arc::new(RwLock::new(MemStorage {
                rooms: HashMap::new(),
                users: Vec::new(),
                access_tokens: HashMap::new(),
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
        &mut self,
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

    async fn verify_password(&mut self, username: &str, password: &str) -> Result<bool, Error> {
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
        &mut self,
        username: &str,
        _device_id: &str,
    ) -> Result<Uuid, Error> {
        let mut db = self.inner.write().await;
        let token = Uuid::new_v4();
        db.access_tokens.insert(token, username.to_string());
        Ok(token)
    }

    async fn delete_access_token(&mut self, token: Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        db.access_tokens.remove(&token);
        Ok(())
    }

    async fn delete_all_access_tokens(&mut self, token: Uuid) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let username = match db.access_tokens.get(&token) {
            Some(v) => v.clone(),
            None => return Ok(()),
        };
        db.access_tokens.retain(|_token, name| *name != username);
        Ok(())
    }

    async fn try_auth(&mut self, token: Uuid) -> Result<Option<String>, Error> {
        let db = self.inner.read().await;
        Ok(db.access_tokens.get(&token).cloned())
    }

    async fn get_profile(&mut self, username: &str) -> Result<Option<UserProfile>, Error> {
        let db = self.inner.read().await;
        Ok(db
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.profile.clone()))
    }

    async fn set_avatar_url(&mut self, username: &str, avatar_url: &str) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let user = db
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or(Error::UserNotFound)?;
        user.profile.avatar_url = Some(avatar_url.to_string());
        Ok(())
    }

    async fn set_display_name(&mut self, username: &str, display_name: &str) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        let user = db
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or(Error::UserNotFound)?;
        user.profile.displayname = Some(display_name.to_string());
        Ok(())
    }

    async fn add_pdus(&mut self, pdus: &[PduV4]) -> Result<(), Error> {
        let mut db = self.inner.write().await;
        for pdu in pdus {
            if pdu.ty == "m.room.create" {
                db.rooms
                    .insert(pdu.room_id.clone(), Room { events: Vec::new() });
            }
            db.rooms
                .get_mut(&pdu.room_id)
                .ok_or(Error::RoomNotFound)?
                .events
                .push(pdu.clone());
        }
        Ok(())
    }

    async fn query_pdus(&mut self, query: EventQuery) -> Result<Option<Vec<PduV4>>, Error> {
        let db = self.inner.read().await;
        let EventQuery {
            query_type,
            room,
            timeout_ms,
            senders,
            not_senders,
            types,
            not_types,
            contains_json,
        } = query;
        let room = match db.rooms.get(&room) {
            Some(v) => v,
            None => return Ok(None),
        };
        let max = room.events.len();
        let base_slice = match &query_type {
            &QueryType::Timeline { from, to } => {
                &room.events[from..to.unwrap_or(max)]
            },
            &QueryType::State { at, .. } => {
                &room.events[0..at.unwrap_or(max)]
            },
        };
        for i in 0..2usize {
            let base_iter = base_slice
                .iter()
                .filter(|e| {
                    if !senders.is_empty() {
                        senders.contains(&e.sender)
                    } else {
                        true
                    }
                })
                .filter(|e| !not_senders.contains(&e.sender))
                .filter(|e| {
                    if !types.is_empty() {
                        types.contains(&e.ty)
                    } else {
                        true
                    }
                })
                .filter(|e| !not_types.contains(&e.ty));
            
            let ret: Vec<PduV4> = match &query_type {
                QueryType::State { state_keys, not_state_keys, .. } => {
                    base_iter.filter(|e| if !state_keys.is_empty() {
                        match &e.state_key {
                            Some(k) if state_keys.contains(&k) => true,
                            _ => false,
                        }
                    } else {
                        true
                    }).filter(|e| match &e.state_key {
                        Some(k) if !not_state_keys.contains(&k) => false,
                        Some(_) => true,
                        None => false
                    }).cloned().collect()
                },
                _ => base_iter.cloned().collect(),
            };
            log::info!("{:?}", ret);
            if !ret.is_empty() {
                return Ok(Some(ret));
            } else {
                if i == 0 {
                    log::info!("waiting...");
                    tokio::time::delay_for(
                        std::time::Duration::from_millis(timeout_ms as u64)
                    ).await;
                    log::info!("waited!");
                } else if i == 1 {
                    break;
                }
            }
        }
        return Ok(Some(Vec::new()))
        //TODO: contains_json
    }

    async fn get_memberships_by_user(
        &mut self,
        user_id: &MatrixId,
    ) -> Result<HashMap<String, Membership>, Self::Error> {
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
        &mut self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<Event>, Self::Error> {
        let db = self.inner.read().await;
        let event = db
            .rooms
            .get(room_id)
            .map(|r| r.events.iter().find(|e| e.hashes.sha256 == event_id))
            .flatten()
            .map(|event| Event {
                room_id: None,
                sender: event.sender.clone(),
                ty: event.ty.clone(),
                state_key: event.state_key.clone(),
                content: event.content.clone(),
                unsigned: event.unsigned.clone(),
                redacts: event.redacts.clone(),
                event_id: Some(event.hashes.sha256.clone()),
                origin_server_ts: Some(event.origin_server_ts),
            });
        Ok(event)
    }

    async fn get_prev_event_ids(
        &mut self,
        room_id: &str,
    ) -> Result<Option<(i64, Vec<String>)>, Self::Error> {
        let db = self.inner.read().await;
        let room = db.rooms.get(room_id);
        match room {
            Some(r) => {
                let depth = r.events.get(r.events.len() - 1).unwrap().depth;
                let mut prev_event_ids = Vec::new();
                for event in r.events.iter().filter(|e| e.depth == depth) {
                    prev_event_ids.push(event.hashes.sha256.clone());
                }
                return Ok(Some((depth, prev_event_ids)));
            }
            None => return Ok(None),
        }
    }

    async fn get_user_account_data(
        &mut self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Self::Error> {
        let db = self.inner.read().await;
        let map = db
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.account_data.clone())
            .unwrap_or(HashMap::new());
        Ok(map)
    }

    async fn print_the_world(&mut self) -> Result<(), Error> {
        let db = self.inner.read().await;
        log::info!("{:#?}", db);
        Ok(())
    }
}
