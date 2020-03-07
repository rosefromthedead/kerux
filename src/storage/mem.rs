use async_trait::async_trait;
use displaydoc::Display;
use serde_json::Value as JsonValue;
use std::{
    collections::HashMap,
    sync::{Arc, PoisonError, RwLock},
};
use uuid::Uuid;

use crate::{
    events::{
        room::{Member, Membership},
        Event, PduV4,
    },
    storage::UserProfile,
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
    /// The lock on the storage has been poisoned because a thread panicked while holding it.
    LockPoisoned,
    /// The specified user was not found.
    UserNotFound,
    /// The specified room was not found.
    RoomNotFound,
    /// A member event in a room was invalid: {0}.
    InvalidMemberEvent(serde_json::Error),
}

impl std::error::Error for Error {}

impl<T> From<PoisonError<T>> for Error {
    fn from(_: PoisonError<T>) -> Self {
        Error::LockPoisoned
    }
}

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
        let mut db = self.inner.write()?;
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
        let db = self.inner.read()?;
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
        let mut db = self.inner.write()?;
        let token = Uuid::new_v4();
        db.access_tokens.insert(token, username.to_string());
        Ok(token)
    }

    async fn delete_access_token(&mut self, token: Uuid) -> Result<(), Error> {
        let mut db = self.inner.write()?;
        db.access_tokens.remove(&token);
        Ok(())
    }

    async fn delete_all_access_tokens(&mut self, token: Uuid) -> Result<(), Error> {
        let mut db = self.inner.write()?;
        let username = match db.access_tokens.get(&token) {
            Some(v) => v.clone(),
            None => return Ok(()),
        };
        db.access_tokens.retain(|_token, name| *name != username);
        Ok(())
    }

    async fn try_auth(&mut self, token: Uuid) -> Result<Option<String>, Error> {
        let db = self.inner.read()?;
        Ok(db.access_tokens.get(&token).cloned())
    }

    async fn get_profile(&mut self, username: &str) -> Result<Option<UserProfile>, Error> {
        let db = self.inner.read()?;
        Ok(db
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.profile.clone()))
    }

    async fn set_avatar_url(&mut self, username: &str, avatar_url: &str) -> Result<(), Error> {
        let mut db = self.inner.write()?;
        let user = db
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or(Error::UserNotFound)?;
        user.profile.avatar_url = Some(avatar_url.to_string());
        Ok(())
    }

    async fn set_display_name(&mut self, username: &str, display_name: &str) -> Result<(), Error> {
        let mut db = self.inner.write()?;
        let user = db
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or(Error::UserNotFound)?;
        user.profile.displayname = Some(display_name.to_string());
        Ok(())
    }

    async fn add_pdus(&mut self, pdus: &[PduV4]) -> Result<(), Error> {
        let mut db = self.inner.write()?;
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

    async fn get_memberships_by_user(
        &mut self,
        user_id: &MatrixId,
    ) -> Result<HashMap<String, Membership>, Self::Error> {
        let db = self.inner.read()?;
        let mut ret = HashMap::new();
        for (room_id, room) in db.rooms.iter() {
            for pdu in room.events.iter().rev() {
                if pdu.ty == "m.room.member" && pdu.state_key.as_deref() == Some(user_id.as_str()) {
                    let content: Member = serde_json::from_value(pdu.content.clone())
                        .map_err(Error::InvalidMemberEvent)?;
                    ret.insert(room_id.clone(), content.membership);
                    break;
                }
            }
        }
        Ok(ret)
    }

    async fn get_membership(
        &mut self,
        user_id: &MatrixId,
        room_id: &str,
    ) -> Result<Option<Membership>, Error> {
        let db = self.inner.read()?;
        let member_event = db
            .rooms
            .get(room_id)
            .map(|r| {
                r.events.iter().rev().find(|e| {
                    e.ty == "m.room.member" && e.state_key.as_deref() == Some(user_id.as_str())
                })
            })
            .flatten();
        match member_event {
            Some(e) => {
                let content: Member =
                    serde_json::from_value(e.content.clone()).map_err(Error::InvalidMemberEvent)?;
                return Ok(Some(content.membership));
            }
            None => Ok(None),
        }
    }

    async fn get_room_member_counts(&mut self, room_id: &str) -> Result<(i64, i64), Error> {
        let db = self.inner.read()?;
        let room = db.rooms.get(room_id);
        match room {
            Some(r) => {
                let mut visited_users = HashSet::new();
                let mut joined_member_count = 0;
                let mut invited_member_count = 0;
                for event in r.events.iter().rev().filter(|e| e.ty == "m.room.member") {
                    let have_visited = visited_users.insert(event.state_key.clone().unwrap());
                    if !have_visited {
                        let content: Member = serde_json::from_value(event.content.clone())
                            .map_err(Error::InvalidMemberEvent)?;
                        match content.membership {
                            Membership::Join => joined_member_count += 1,
                            Membership::Invite => invited_member_count += 1,
                            _ => {}
                        }
                    }
                }
                Ok((joined_member_count, invited_member_count))
            }
            None => return Ok((0, 0)),
        }
    }

    async fn get_full_state(&mut self, room_id: &str) -> Result<Vec<Event>, Error> {
        let db = self.inner.read()?;
        let room = db.rooms.get(room_id);
        match room {
            Some(r) => {
                let mut ret = Vec::new();
                let mut visited_state = HashSet::new();
                for event in r.events.iter().rev().filter(|e| e.state_key.is_some()) {
                    let is_unique =
                        visited_state.insert((event.ty.clone(), event.state_key.clone().unwrap()));
                    if is_unique {
                        ret.push(Event {
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
                    }
                }
                return Ok(ret);
            }
            None => return Ok(Vec::new()),
        }
    }

    async fn get_state_event(
        &mut self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<Option<Event>, Error> {
        let db = self.inner.read()?;
        let event = db
            .rooms
            .get(room_id)
            .map(|r| {
                r.events
                    .iter()
                    .find(|e| e.ty == event_type && e.state_key.as_deref() == Some(state_key))
            })
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
        return Ok(event);
    }

    async fn get_events_since(
        &mut self,
        room_id: &str,
        since: u64,
    ) -> Result<Vec<Event>, Self::Error> {
        let db = self.inner.read()?;
        let room = db.rooms.get(room_id);
        let mut ret = Vec::new();
        match room {
            Some(r) => {
                for event in &r.events[since as usize..] {
                    ret.push(Event {
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
                }
                return Ok(ret);
            }
            None => return Ok(Vec::new()),
        }
    }

    async fn get_event(
        &mut self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<Event>, Self::Error> {
        let db = self.inner.read()?;
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
        let db = self.inner.read()?;
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
        let db = self.inner.read()?;
        let map = db
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.account_data.clone())
            .unwrap_or(HashMap::new());
        Ok(map)
    }

    async fn print_the_world(&mut self) -> Result<(), Error> {
        let db = self.inner.read()?;
        log::info!("{:#?}", db);
        Ok(())
    }
}
