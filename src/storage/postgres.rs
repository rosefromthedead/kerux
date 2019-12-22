use crossbeam::queue::ArrayQueue;
use futures::{
    compat::{Future01CompatExt, Stream01CompatExt},
    stream::{StreamExt, TryStreamExt},
    FutureExt,
};
use pg::{Client, Error as DbError, NoTls};
use serde_json::{Value as JsonValue, to_value as to_json_value};
use std::{
    collections::HashMap,
    sync::Arc
};

use crate::{
    client::error::Error,
    events::{room::Membership, PduV4, Event},
};

pub struct DbPool {
    db_address: String,
    queue: Arc<ArrayQueue<Client>>,
}

pub struct ClientGuard {
    queue: Arc<ArrayQueue<Client>>,
    inner: Option<Client>,
}

impl DbPool {
    pub fn new(db_address: String, cap: usize) -> Self {
        DbPool {
            db_address,
            queue: Arc::new(ArrayQueue::new(cap)),
        }
    }

    pub async fn get_client(&self) -> Result<ClientGuard, pg::Error> {
        if let Ok(client) = self.queue.pop() {
            return Ok(ClientGuard {
                queue: Arc::clone(&self.queue),
                inner: Some(client),
            });
        } else {
            let (client, conn) = pg::connect(&*self.db_address, NoTls).compat().await.map_err(|e| {
                log::warn!("{}", e); e
            })?;
            tokio::spawn(conn.compat().map(|_| ()));
            return Ok(ClientGuard {
                queue: Arc::clone(&self.queue),
                inner: Some(client),
            });
        }
    }
}

impl std::fmt::Debug for DbPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DbPool {{ db_address: \"{}\", ... }}", self.db_address)?;
        Ok(())
    }
}
/*
impl std::ops::Deref for ClientGuard {
    type Target = Client;

    fn deref(&self) -> &Client {
        self.inner.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for ClientGuard {
    fn deref_mut(&mut self) -> &mut Client {
        self.inner.as_mut().unwrap()
    }
}
*/
impl Drop for ClientGuard {
    fn drop(&mut self) {
        let _ = self.queue.push(self.inner.take().unwrap());
    }
}

// Eventually this will become a trait to allow more storage backends
impl ClientGuard {
    pub async fn create_user(&mut self, username: &str, password_hash: Option<&str>) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("INSERT INTO users(id, password_hash) VALUES ($1, $2);").compat().await?;
        db.execute(&stmt, &[&username, &password_hash]).compat().await?;
        Ok(())
    }

    pub async fn verify_password(&mut self, username: &str, password: &str) -> Result<bool, DbError> {
        let db = self.inner.as_mut().unwrap();
        let get_user = db.prepare("SELECT password_hash FROM users WHERE id=$1;").compat().await?;
        let mut rows = db.query(&get_user, &[&username]).compat(); 
        let user = match rows.next().await {
            Some(v) => v?,
            None => return Ok(false),
        };

        match argon2::verify_encoded(user.try_get("password_hash")?, password.as_bytes()) {
            Ok(true) => Ok(true),
            Ok(false) | Err(_) => Ok(false),
        }
    }

    pub async fn create_access_token(&mut self, username: &str, device_id: &str) -> Result<uuid::Uuid, DbError> {
        let db = self.inner.as_mut().unwrap();
        let access_token = uuid::Uuid::new_v4();
        let insert_token = db.prepare("INSERT INTO access_tokens(token, username, device_id) VALUES ($1, $2, $3);").compat().await?;
        db.execute(&insert_token, &[&access_token, &username, &device_id]).compat().await?;
        Ok(access_token)
    }

    pub async fn delete_access_token(&mut self, token: uuid::Uuid) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("DELETE FROM access_tokens WHERE token = $1;").compat().await?;
        db.execute(&stmt, &[&token]).compat().await?;
        Ok(())
    }

    pub async fn delete_all_access_tokens(&mut self, token: uuid::Uuid) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("
            WITH name AS (
                SELECT username FROM access_tokens WHERE token = $1
            )
            DELETE FROM access_tokens WHERE username IN name;
        ").compat().await?;
        db.execute(&stmt, &[&token]).compat().await?;
        Ok(())
    }

    pub async fn try_auth(&mut self, token: uuid::Uuid) -> Result<String, Error> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("SELECT username FROM access_tokens WHERE token = $1;").compat().await?;
        let mut rows = db.query(&query, &[&token]).compat();
        let row = rows.next().await.ok_or(Error::UnknownToken)??;
        let id = row.get("username");
        Ok(id)
    }

    /// Returns the given user's avatar URL and display name, if present
    pub async fn get_profile(&mut self, username: &str) -> Result<(Option<String>, Option<String>), Error> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("SELECT avatar_url, display_name FROM users WHERE id = $1;").compat().await?;
        let mut rows = db.query(&query, &[&username]).compat();
        let row = match rows.next().await {
            Some(v) => v?,
            None => return Err(Error::NotFound),
        };
        let avatar_url: Option<String> = row.try_get("avatar_url")?;
        let display_name: Option<String> = row.try_get("display_name")?;
        Ok((avatar_url, display_name))
    }

    pub async fn set_avatar_url(&mut self, username: &str, avatar_url: &str) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("UPDATE users SET avatar_url = $1 WHERE id = $2;").compat().await?;
        db.execute(&stmt, &[&avatar_url, &username]).compat().await?;
        Ok(())
    }

    pub async fn set_display_name(&mut self, username: &str, display_name: &str) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("UPDATE users SET display_name = $1 WHERE id = $2;").compat().await?;
        db.execute(&stmt, &[&display_name, &username]).compat().await?;
        Ok(())
    }

    pub async fn add_events(&mut self,
        events: impl IntoIterator<Item = &PduV4>
    ) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("
            INSERT INTO room_events(room_id, sender, origin, origin_server_ts, type, state_key,
                    content, prev_events, depth, auth_events, redacts, unsigned, hash,
                    signatures)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14);
        ").compat().await?;
        for event in events {
            db.execute(&stmt, &[
                &event.room_id,
                &event.sender,
                &event.origin,
                &event.origin_server_ts,
                &event.ty,
                &event.state_key,
                &JsonValue::from(event.content.clone()),
                &to_json_value(&event.prev_events).unwrap(),
                &event.depth,
                &to_json_value(&event.auth_events).unwrap(),
                &event.redacts,
                &event.unsigned.as_ref().map(to_json_value).map(Result::unwrap),
                &event.hashes.sha256,
                &to_json_value(&event.signatures).unwrap(),
            ]).compat().await?;
            handle_event(db, &event).await?;
        }
        Ok(())
    }

    pub async fn get_memberships_by_user(&mut self, user_id: &str)
            -> Result<Vec<(String, Membership)>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT room_id, membership FROM room_memberships WHERE user_id = $1;
        ").compat().await?;
        db.query(&query, &[&user_id]).compat()
            .map(|row| {
                let row = row?;
                let membership_str: &str = row.get("membership");
                let membership = membership_str.parse().unwrap();
                Ok((row.get("room_id"), membership))
            })
            .try_collect::<Vec<(_, _)>>().await
    }

    pub async fn get_membership(&mut self, user_id: &str, room_id: &str) 
            -> Result<Option<Membership>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT membership
                FROM room_memberships
                WHERE user_id = $1 AND room_id = $2;
        ").compat().await?;
        let mut rows = db.query(&query, &[&user_id, &room_id]).compat();
        match rows.next().await {
            Some(row) => {
                let row = row?;
                let membership_str: &str = row.get("membership");
                let membership = membership_str.parse().unwrap();
                Ok(Some(membership))
            },
            None => Ok(None),
        }
    }

    pub async fn get_room_member_counts(&mut self, room_id: &str) -> Result<(i64, i64), DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT membership, COUNT(membership) FROM room_memberships
                WHERE room_id = $1
                GROUP BY membership;
        ").compat().await?;
        let mut rows = db.query(&query, &[&room_id]).compat();
        let mut joined_member_count = 0;
        let mut invited_member_count = 0;
        while let Some(row) = rows.next().await {
            let row = row?;
            let count = row.get("count");
            match row.get("membership") {
                "join" => joined_member_count = count,
                "invite" => invited_member_count = count,
                _ => {},
            }
        }
        Ok((joined_member_count, invited_member_count))
    }

    pub async fn get_full_state(&mut self, room_id: &str) -> Result<Vec<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT DISTINCT ON (type, state_key)
                content, type, hash, sender, origin_server_ts, unsigned, state_key
                FROM room_events
                WHERE room_id = $1 AND state_key != NULL
                ORDER BY type, state_key, depth DESC;
        ").compat().await?;
        let mut rows = db.query(&query, &[&room_id]).compat();
        let mut ret = Vec::new();
        while let Some(row) = rows.next().await {
            let row = row?;
            ret.push(Event {
                content: row.get("content"),
                ty: row.get("type"),
                event_id: row.get("hash"),
                room_id: None,
                sender: row.get("sender"),
                origin_server_ts: row.get("origin_server_ts"),
                unsigned: row.get("unsigned"),
                state_key: row.get("state_key"),
            });
        }
        Ok(ret)
    }

    pub async fn get_state_event(&mut self, room_id: &str, event_type: &str, state_key: &str)
            -> Result<Option<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT DISTINCT ON (state_key)
                content, type, hash, sender, origin_server_ts, unsigned, state_key
                FROM room_events
                WHERE room_id = $1 AND type = $2 AND state_key = $3
                ORDER BY state_key, depth DESC;
        ").compat().await?;
        let mut rows = db.query(&query, &[&room_id, &event_type, &state_key]).compat();
        if let Some(row) = rows.next().await {
            let row = row?;
            Ok(Some(Event {
                content: row.get("content"),
                ty: row.get("type"),
                event_id: row.get("hash"),
                room_id: None,
                sender: row.get("sender"),
                origin_server_ts: row.get("origin_server_ts"),
                unsigned: row.get("unsigned"),
                state_key: row.get("state_key"),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_events_since(&mut self, room_id: &str, since: Option<&str>)
            -> Result<Vec<Event>, Error> {
        let db = self.inner.as_mut().unwrap();
        let since: i64 = since.map(str::parse)
            .unwrap_or(Ok(0))
            .map_err(|_| Error::InvalidParam(String::from("invalid since param")))?;
        let query = db.prepare("
            SELECT
                content, type, hash, sender, origin_server_ts, unsigned, state_key
                FROM room_events
                WHERE room_id = $1 AND ordering > $2;
        ").compat().await?;
        let mut rows = db.query(&query, &[&room_id, &since]).compat();
        let mut ret = Vec::new();
        while let Some(row) = rows.next().await {
            let row = row?;
            ret.push(Event {
                content: row.get("content"),
                ty: row.get("type"),
                event_id: row.get("hash"),
                room_id: None,
                sender: row.get("sender"),
                origin_server_ts: row.get("origin_server_ts"),
                unsigned: row.get("unsigned"),
                state_key: row.get("state_key"),
            });
        }
        Ok(ret)
    }

    pub async fn get_event(&mut self, room_id: &str, event_id: &str)
            -> Result<Option<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT
                content, type, hash, room_id, sender, origin_server_ts, unsigned, state_key
                FROM room_events
                WHERE room_id = $1 AND event_id = $2;
        ").compat().await?;
        let mut rows = db.query(&query, &[&room_id, &event_id]).compat();
        if let Some(row) = rows.next().await {
            let row = row?;
            Ok(Some(Event {
                content: row.get("content"),
                ty: row.get("type"),
                event_id: row.get("hash"),
                room_id: Some(row.get("room_id")),
                sender: row.get("sender"),
                origin_server_ts: row.get("origin_server_ts"),
                unsigned: row.get("unsigned"),
                state_key: row.get("state_key"),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_prev_event_ids(&mut self, room_id: &str)
            -> Result<Option<(i64, Vec<String>)>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT depth, hash FROM room_events 
                WHERE depth = (SELECT MAX(depth) FROM room_events WHERE room_id = $1);
        ").compat().await?;
        let mut rows = db.query(&query, &[&room_id]).compat();
        let mut ret = Vec::new();
        let mut depth = 0;
        while let Some(row) = rows.next().await {
            let row = row?;
            ret.push(row.get("hash"));
            depth = row.get("depth");   // same every time but oh well
        }
        if ret.len() != 0 {
            return Ok(Some((depth, ret)));
        } else {
            return Ok(None);
        }
    }

    pub async fn get_user_account_data(&mut self, username: &str)
            -> Result<HashMap<String, JsonValue>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT (type, content) FROM user_account_data WHERE username = $1;
        ").compat().await?;
        let mut rows = db.query(&query, &[&username]).compat();
        let mut ret = HashMap::new();
        while let Some(row) = rows.next().await {
            let row = row?;
            ret.insert(row.get("type"), row.get("content"));
        }
        Ok(ret)
    }
}

async fn handle_event(db: &mut Client, event: &PduV4) -> Result<(), DbError> {
    match &*event.ty {
        "m.room.member" => {
            let membership = event.content.get("membership").unwrap().as_str().unwrap();
            if membership != "leave" {
                let stmt = db.prepare("
                    INSERT INTO room_memberships(user_id, room_id, membership, event_id)
                        VALUES ($1, $2, $3, $4)
                        ON CONFLICT(user_id, room_id) DO UPDATE SET membership = $3, event_id = $4;
                ").compat().await?;
                db.execute(&stmt,
                    &[&event.state_key, &event.room_id, &membership, &event.hashes.sha256]
                ).compat().await?;
            } else {
                let stmt = db.prepare("
                    DELETE FROM room_memberships WHERE user_id = $1 AND room_id = $2;
                ").compat().await?;
                db.execute(&stmt, &[&event.state_key, &event.room_id]).compat().await?;
            }
        },
        "m.room.create" => {
            let stmt = db.prepare("
                INSERT INTO rooms(id)
                    VALUES ($1);
            ").compat().await?;
            db.execute(&stmt, &[&event.room_id]).compat().await?;
        }
        _ => {},
    }
    Ok(())
}
