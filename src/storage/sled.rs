use std::{
    collections::HashMap,
    convert::TryInto,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bincode::{DefaultOptions, Options};
use itertools::Itertools;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sled::{
    transaction::{ConflictableTransactionError, TransactionalTree},
    Db, IVec, Tree,
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    error::{Error, ErrorKind},
    events::{ephemeral::Typing, pdu::StoredPdu},
    storage::{Storage, StorageManager},
    util::MatrixId,
};

use super::{Batch, EventQuery, QueryType, UserProfile};

trait TreeExt {
    type Error;
    fn get_value<K: AsRef<[u8]>, V: DeserializeOwned>(
        &self,
        key: K,
    ) -> Result<Option<V>, Self::Error>;
    fn replace_value<K: AsRef<[u8]>, V: DeserializeOwned + Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<Option<V>, Self::Error>;
    /// Returns whether the insert succeeded (i.e. the key was not already present)
    fn try_insert_value<K: AsRef<[u8]>, V: Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<bool, Self::Error>;
    /// Returns whether something was overwritten (i.e. the key was already present)
    fn overwrite_value<K: AsRef<[u8]>, V: Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<bool, Self::Error>;
}

impl TreeExt for Tree {
    type Error = Error;
    fn get_value<K: AsRef<[u8]>, V: DeserializeOwned>(&self, key: K) -> Result<Option<V>, Error> {
        self.get(key)?
            .map(|bytes| DefaultOptions::new().deserialize(&bytes))
            .transpose()
            .map_err(Into::into)
    }

    fn replace_value<K: AsRef<[u8]>, V: DeserializeOwned + Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<Option<V>, Error> {
        let bytes = DefaultOptions::new().serialize(&value)?;
        self.insert(key, &*bytes)?
            .map(|bytes| DefaultOptions::new().deserialize(&bytes))
            .transpose()
            .map_err(Into::into)
    }

    fn try_insert_value<K: AsRef<[u8]>, V: Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<bool, Error> {
        let bytes = DefaultOptions::new().serialize(&value)?;
        if self.contains_key(&key)? {
            return Ok(false);
        }
        self.insert(key, &*bytes)?;
        Ok(true)
    }

    fn overwrite_value<K: AsRef<[u8]>, V: Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<bool, Error> {
        let bytes = DefaultOptions::new().serialize(&value)?;
        let was_there = self.insert(key, &*bytes)?.is_some();
        Ok(was_there)
    }
}

impl TreeExt for TransactionalTree {
    type Error = ConflictableTransactionError<bincode::Error>;
    fn get_value<K: AsRef<[u8]>, V: DeserializeOwned>(
        &self,
        key: K,
    ) -> Result<Option<V>, Self::Error> {
        self.get(key)?
            .map(|bytes| DefaultOptions::new().deserialize(&bytes))
            .transpose()
            .map_err(ConflictableTransactionError::Abort)
    }

    fn replace_value<K: AsRef<[u8]>, V: DeserializeOwned + Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<Option<V>, Self::Error> {
        let bytes = DefaultOptions::new()
            .serialize(&value)
            .map_err(ConflictableTransactionError::Abort)?;
        self.insert(IVec::from(key.as_ref()), &*bytes)?
            .map(|bytes| DefaultOptions::new().deserialize(&bytes))
            .transpose()
            .map_err(ConflictableTransactionError::Abort)
    }

    fn try_insert_value<K: AsRef<[u8]>, V: Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<bool, Self::Error> {
        let bytes = DefaultOptions::new()
            .serialize(&value)
            .map_err(ConflictableTransactionError::Abort)?;
        if self.get(&key)?.is_some() {
            return Ok(false);
        }
        self.insert(IVec::from(key.as_ref()), &*bytes)?;
        Ok(true)
    }

    fn overwrite_value<K: AsRef<[u8]>, V: Serialize>(
        &self,
        key: K,
        value: V,
    ) -> Result<bool, Self::Error> {
        let bytes = DefaultOptions::new()
            .serialize(&value)
            .map_err(ConflictableTransactionError::Abort)?;
        let was_there = self.insert(IVec::from(key.as_ref()), &*bytes)?.is_some();
        Ok(was_there)
    }
}

#[derive(Default, Deserialize, Serialize)]
struct User {
    password_hash: String,
    profile: UserProfile,
    account_data: HashMap<String, JsonValue>,
}

#[derive(Deserialize, Serialize)]
struct AccessTokenData {
    username: String,
    device_id: String,
}

#[derive(Default)]
struct Ephemeral {
    ephemeral: HashMap<String, JsonValue>,
    typing: HashMap<MatrixId, Instant>,
}

impl Ephemeral {
    fn get_typing(&self) -> Typing {
        let now = Instant::now();
        let mut ret = Typing::default();
        for (mxid, _) in self.typing.iter().filter(|(_, timeout)| **timeout > now) {
            ret.user_ids.insert(mxid.clone());
        }
        ret
    }
}

pub struct SledStorage(SledStorageHandle);

impl SledStorage {
    pub fn new(path: &str) -> Result<Self, Error> {
        let db = sled::open(path)?;
        Ok(Self(SledStorageHandle {
            all: db.clone(),
            events: db.open_tree("events")?,
            rooms: db.open_tree("rooms")?,
            users: db.open_tree("users")?,
            access_tokens: db.open_tree("access_tokens")?,
            txn_ids: db.open_tree("txn_ids")?,
            batches: db.open_tree("batches")?,
            room_orderings: Arc::new(Mutex::new(HashMap::new())),
            headless_events: db.open_tree("headless_events")?,
            ephemeral: Arc::new(Mutex::new(HashMap::new())),
        }))
    }
}

#[async_trait]
impl StorageManager for SledStorage {
    async fn get_handle(&self) -> Result<Box<dyn Storage>, Error> {
        Ok(Box::new(self.0.clone()))
    }
}

#[derive(Clone)]
pub struct SledStorageHandle {
    all: Db,
    events: Tree,
    rooms: Tree,
    users: Tree,
    access_tokens: Tree,
    txn_ids: Tree,
    batches: Tree,
    room_orderings: Arc<Mutex<HashMap<String, Tree>>>,
    headless_events: Tree,
    ephemeral: Arc<Mutex<HashMap<String, Ephemeral>>>,
}

impl SledStorageHandle {
    async fn get_room_ordering_tree(&self, room_id: &str) -> Result<Tree, Error> {
        let mut ordering_trees = self.room_orderings.lock().await;
        if let Some(tree) = ordering_trees.get(room_id) {
            Ok(tree.clone())
        } else {
            let tree = self.all.open_tree(room_id)?;
            ordering_trees.insert(room_id.to_string(), tree.clone());
            Ok(tree)
        }
    }

    async fn get_events(
        &self,
        ordering_tree: &Tree,
        query: &EventQuery<'_>,
        from: usize,
        to: Option<usize>,
    ) -> Result<(Vec<StoredPdu>, usize), Error> {
        let mut ret = Vec::new();

        let from_bytes = from.to_be_bytes();
        let to_bytes = to.clone().map(usize::to_be_bytes);
        let pdu_iter = match to_bytes {
            Some(to_bytes) => ordering_tree.range(from_bytes..=to_bytes),
            None => ordering_tree.range(from_bytes..),
        }
        .map_ok(|(_key, event_id)| {
            self.events.get(&format!(
                "{}_{}",
                query.room_id,
                String::from_utf8(Vec::from(event_id.as_ref())).unwrap()
            ))
        })
        // flatten
        .map(|res| match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) | Err(e) => Err(e),
        });
        for pdu in pdu_iter {
            // is Ok(None) if the event is not present, but it must be present if it's in the
            // ordering tree
            let pdu: StoredPdu = DefaultOptions::new().deserialize(pdu?.unwrap().as_ref())?;
            if query.matches(&pdu.inner()) {
                ret.push(pdu);
            }
        }
        Ok((ret, to.unwrap()))
    }
}

#[async_trait]
impl Storage for SledStorageHandle {
    async fn create_user(&self, username: &str, password: &str) -> Result<(), Error> {
        let salt: [u8; 16] = rand::random();
        let password_hash = argon2::hash_encoded(password.as_bytes(), &salt, &Default::default())?;
        let did_insert = self.users.try_insert_value(
            username,
            &User {
                password_hash: password_hash.to_string(),
                ..Default::default()
            },
        )?;
        match did_insert {
            true => Ok(()),
            false => Err(ErrorKind::UsernameTaken.into()),
        }
    }

    async fn verify_password(&self, username: &str, password: &str) -> Result<bool, Error> {
        let user: Option<User> = self.users.get_value(username)?;
        if let Some(user) = user {
            match argon2::verify_encoded(&user.password_hash, password.as_bytes()) {
                Ok(true) => Ok(true),
                Ok(false) => Ok(false),
                Err(_) => Ok(false),
            }
        } else {
            Ok(false)
        }
    }

    async fn create_access_token(&self, username: &str, device_id: &str) -> Result<Uuid, Error> {
        let token = Uuid::new_v4();
        if !self.users.contains_key(username)? {
            return Err(ErrorKind::UserNotFound.into());
        }
        self.access_tokens.try_insert_value(
            token.as_bytes(),
            &AccessTokenData {
                username: username.to_string(),
                device_id: device_id.to_string(),
            },
        )?;
        Ok(token)
    }

    async fn delete_access_token(&self, token: Uuid) -> Result<(), Error> {
        self.access_tokens.remove(token.as_bytes())?;
        Ok(())
    }

    async fn delete_all_access_tokens(&self, token: Uuid) -> Result<(), Error> {
        let data: Option<AccessTokenData> = self.access_tokens.get_value(token.as_bytes())?;
        if let Some(data) = data {
            let username = data.username;
            let iter = (&self.access_tokens).into_iter();
            let mut to_delete = Vec::new();
            for res in iter {
                let (key, val) = res?;
                let data = DefaultOptions::new()
                    .deserialize::<AccessTokenData>(&val)
                    .unwrap();
                if data.username == username {
                    to_delete.push(key);
                }
            }
            for key in to_delete.into_iter() {
                self.access_tokens.remove(key)?;
            }
        }
        Ok(())
    }

    async fn try_auth(&self, token: Uuid) -> Result<Option<String>, Error> {
        let maybe_username = self
            .access_tokens
            .get_value(token.as_bytes())?
            .map(|data: AccessTokenData| data.username);
        Ok(maybe_username)
    }

    async fn record_txn(&self, token: Uuid, txn_id: String) -> Result<bool, Error> {
        let name = format!("{}_{}", token, txn_id);
        let is_new = self.txn_ids.insert(&name, &[])?.is_none();
        Ok(is_new)
    }

    async fn get_profile(&self, username: &str) -> Result<Option<UserProfile>, Error> {
        let profile = self.users.get_value(username)?.map(|u: User| u.profile);
        Ok(profile)
    }

    async fn set_avatar_url(&self, username: &str, avatar_url: &str) -> Result<(), Error> {
        let mut user: User = self
            .users
            .get_value(username)?
            .ok_or(ErrorKind::UserNotFound)?;
        user.profile.avatar_url = Some(avatar_url.to_string());
        self.users.overwrite_value(username, user)?;
        Ok(())
    }

    async fn set_display_name(&self, username: &str, display_name: &str) -> Result<(), Error> {
        let mut user: User = self
            .users
            .get_value(username)?
            .ok_or(ErrorKind::UserNotFound)?;
        user.profile.displayname = Some(display_name.to_string());
        self.users.overwrite_value(username, user)?;
        Ok(())
    }

    async fn add_pdus(&self, pdus: &[StoredPdu]) -> Result<(), Error> {
        for pdu in pdus {
            let name = format!("{}_{}", pdu.room_id(), pdu.event_id());
            self.events.try_insert_value(name, pdu)?;
            let ordering_tree = self.get_room_ordering_tree(&pdu.room_id()).await?;
            'cas: loop {
                if let Some((key, _value)) = ordering_tree.last()? {
                    let idx = u32::from_be_bytes(key[0..4].try_into().unwrap()) + 1;
                    let res = ordering_tree.compare_and_swap(
                        &u32::to_be_bytes(idx),
                        Option::<&[u8]>::None,
                        Some(&*pdu.event_id()),
                    )?;
                    if res.is_ok() {
                        break 'cas;
                    }
                }
            }
            for prev_event in pdu.prev_events() {
                self.headless_events
                    .remove(&format!("{}~{}", pdu.room_id(), prev_event))?;
            }
            self.headless_events
                .insert(&format!("{}~{}", pdu.room_id(), pdu.event_id()), &[])?;
            self.rooms.insert(pdu.room_id().clone(), &[])?;
        }
        Ok(())
    }

    async fn get_prev_events(&self, room_id: &str) -> Result<(Vec<String>, i64), Error> {
        let max_depth: i64 = self.headless_events.get_value(room_id)?.unwrap_or(-1);
        let mut prefix = String::from(room_id).into_bytes();
        prefix.push(b'~');
        self.headless_events
            .scan_prefix(&prefix)
            .keys()
            .map_ok(|k| k.split(|&b| b == b'~').nth(1).unwrap().to_owned())
            .map_ok(String::from_utf8)
            .map_ok(Result::unwrap)
            .collect::<Result<Vec<String>, sled::Error>>()
            .map_err(ErrorKind::SledError)
            .map_err(ErrorKind::into)
            .map(|prev_events| (prev_events, max_depth))
    }

    async fn query_pdus<'a>(
        &self,
        query: EventQuery<'a>,
        wait: bool,
    ) -> Result<(Vec<StoredPdu>, usize), Error> {
        let ordering_tree = self.get_room_ordering_tree(&query.room_id).await?;
        if ordering_tree.is_empty() {
            return Err(ErrorKind::RoomNotFound.into());
        }

        let (mut from, mut to) = match &query.query_type {
            &QueryType::Timeline { from, to } => (from, to),
            &QueryType::State { at, .. } => (0, at),
        };

        let res = self.get_events(&ordering_tree, &query, from, to).await?;

        // if we don't need to wait, return asap
        if !(wait && res.0.is_empty() && query.query_type.is_timeline()) {
            return Ok(res);
        }

        self.events.watch_prefix(&query.room_id).await;
        from = to.unwrap();
        to = None;

        // this time we roll with it
        self.get_events(&ordering_tree, &query, from, to).await
    }

    async fn get_rooms(&self) -> Result<Vec<String>, Error> {
        self.rooms
            .iter()
            .map_ok(|(key, _value)| String::from_utf8(Vec::from(key.as_ref())).unwrap())
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    async fn get_pdu(&self, room_id: &str, event_id: &str) -> Result<Option<StoredPdu>, Error> {
        self.events
            .get_value(&format!("{}_{}", room_id, event_id))
            .map_err(Into::into)
    }

    async fn get_all_ephemeral(&self, room_id: &str) -> Result<HashMap<String, JsonValue>, Error> {
        //TODO: this inserts an ephemeral entry even if the room doesn't actually exist - figure
        // out what to do about it
        let mut ephemerals = self.ephemeral.lock().await;
        let ephemeral = ephemerals.entry(String::from(room_id)).or_default();
        let mut ret = ephemeral.ephemeral.clone();
        ret.insert(
            String::from("m.typing"),
            serde_json::to_value(ephemeral.get_typing()).unwrap(),
        );
        Ok(ret)
    }

    async fn get_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> Result<Option<JsonValue>, Error> {
        let mut ephemerals = self.ephemeral.lock().await;
        let ephemeral = ephemerals.entry(String::from(room_id)).or_default();
        if event_type == "m.typing" {
            let typing = ephemeral.get_typing();
            match typing.user_ids.is_empty() {
                true => Ok(None),
                false => Ok(Some(serde_json::to_value(typing)?)),
            }
        } else {
            Ok(ephemeral.ephemeral.get(event_type).cloned())
        }
    }

    async fn set_ephemeral(
        &self,
        room_id: &str,
        event_type: &str,
        content: Option<JsonValue>,
    ) -> Result<(), Error> {
        assert!(
            event_type != "m.typing",
            "m.typing should not be set directly"
        );
        let mut ephemerals = self.ephemeral.lock().await;
        let ephemeral = ephemerals.entry(String::from(room_id)).or_default();
        match content {
            Some(c) => ephemeral.ephemeral.insert(String::from(event_type), c),
            None => ephemeral.ephemeral.remove(event_type),
        };
        Ok(())
    }

    async fn set_typing(
        &self,
        room_id: &str,
        user_id: &MatrixId,
        is_typing: bool,
        timeout: u32,
    ) -> Result<(), Error> {
        let mut ephemerals = self.ephemeral.lock().await;
        let ephemeral = ephemerals.entry(String::from(room_id)).or_default();
        if is_typing {
            ephemeral.typing.insert(
                user_id.clone(),
                Instant::now() + Duration::from_millis(timeout as u64),
            );
        } else {
            ephemeral.typing.remove(user_id);
        }

        Ok(())
    }

    async fn get_user_account_data(
        &self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Error> {
        let user: User = self
            .users
            .get_value(username)?
            .ok_or(ErrorKind::UserNotFound)?;
        Ok(user.account_data.clone())
    }

    async fn get_batch(&self, id: &str) -> Result<Option<Batch>, Error> {
        self.batches.get_value(id)
    }

    async fn set_batch(&self, id: &str, batch: Batch) -> Result<(), Error> {
        self.batches.overwrite_value(id, batch).map(drop)
    }
}
