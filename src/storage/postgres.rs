use async_trait::async_trait;
use crossbeam::queue::ArrayQueue;
use futures::{
    FutureExt,
};
use pg::{Client, Error as DbError, NoTls};
use serde_json::{Value as JsonValue, to_value as to_json_value};
use std::{
    collections::HashMap,
    sync::Arc
};

use crate::{
    events::{room::Membership, PduV4, Event},
    storage::UserProfile,
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
}

#[async_trait]
impl super::StorageManager for DbPool {
    type Handle = ClientGuard;
    type Error = pg::Error;

    async fn get_handle(&self) -> Result<ClientGuard, pg::Error> {
        if let Ok(client) = self.queue.pop() {
            return Ok(ClientGuard {
                queue: Arc::clone(&self.queue),
                inner: Some(client),
            });
        } else {
            let (client, conn) = pg::connect(&*self.db_address, NoTls).await.map_err(|e| {
                log::warn!("{}", e); e
            })?;
            tokio::spawn(conn.map(|_| ()));
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

#[async_trait]
impl super::Storage for ClientGuard {
    type Error = pg::Error;

    async fn create_user(&mut self, username: &str, password_hash: Option<&str>) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("INSERT INTO users(id, password_hash) VALUES ($1, $2);").await?;
        db.execute(&stmt, &[&username, &password_hash]).await?;
        Ok(())
    }

    async fn verify_password(&mut self, username: &str, password: &str) -> Result<bool, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query(
            "SELECT password_hash FROM users WHERE id=$1;", &[&username]).await?;
        let user = match rows.get(0) {
            Some(v) => v,
            None => return Ok(false),
        };

        match argon2::verify_encoded(user.get("password_hash"), password.as_bytes()) {
            Ok(true) => Ok(true),
            Ok(false) | Err(_) => Ok(false),
        }
    }

    async fn create_access_token(&mut self, username: &str, device_id: &str) -> Result<uuid::Uuid, DbError> {
        let db = self.inner.as_mut().unwrap();
        let access_token = uuid::Uuid::new_v4();
        db.execute(
            "INSERT INTO access_tokens(token, username, device_id) VALUES ($1, $2, $3);",
            &[&access_token, &username, &device_id]
        ).await?;
        Ok(access_token)
    }

    async fn delete_access_token(&mut self, token: uuid::Uuid) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        db.execute("DELETE FROM access_tokens WHERE token = $1;", &[&token]).await?;
        Ok(())
    }

    async fn delete_all_access_tokens(&mut self, token: uuid::Uuid) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        db.execute("
            WITH name AS (
                SELECT username FROM access_tokens WHERE token = $1
            )
            DELETE FROM access_tokens WHERE username IN name;
        ", &[&token]).await?;
        Ok(())
    }

    async fn try_auth(&mut self, token: uuid::Uuid) -> Result<Option<String>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query(
            "SELECT username FROM access_tokens WHERE token = $1;", &[&token]
        ).await?;
        let id = match rows.get(0) {
            Some(r) => r.get("username"),
            None => None,
        };
        Ok(id)
    }

    async fn get_profile(&mut self, username: &str) -> Result<Option<UserProfile>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query(
            "SELECT avatar_url, display_name FROM users WHERE id = $1;", &[&username]
        ).await?;
        let row = match rows.get(0) {
            Some(v) => v,
            None => return Ok(None),
        };
        let profile = UserProfile {
            avatar_url: row.try_get("avatar_url")?,
            displayname: row.try_get("display_name")?,
        };
        Ok(Some(profile))
    }

    async fn set_avatar_url(&mut self, username: &str, avatar_url: &str) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        db.execute(
            "UPDATE users SET avatar_url = $1 WHERE id = $2;", &[&avatar_url, &username]
        ).await?;
        Ok(())
    }

    async fn set_display_name(&mut self, username: &str, display_name: &str) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        db.execute(
            "UPDATE users SET display_name = $1 WHERE id = $2;", &[&display_name, &username]
        ).await?;
        Ok(())
    }

    async fn add_pdus(
        &mut self,
        pdus: &[PduV4],
    ) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("
            INSERT INTO room_events(room_id, sender, origin, origin_server_ts, type, state_key,
                    content, prev_events, depth, auth_events, redacts, unsigned, hash,
                    signatures)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14);
        ").await?;
        for event in pdus {
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
            ]).await?;
            handle_event(db, &event).await?;
        }
        Ok(())
    }

    async fn get_memberships_by_user(&mut self, user_id: &str)
            -> Result<HashMap<String, Membership>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT room_id, membership FROM room_memberships WHERE user_id = $1;
        ").await?;
        let memberships = db.query(&query, &[&user_id]).await?.iter()
            .map(|row| {
                let membership_str: &str = row.get("membership");
                let membership = membership_str.parse().unwrap();
                (row.get("room_id"), membership)
            })
            .collect::<HashMap<_, _>>();
        Ok(memberships)
    }

    async fn get_membership(&mut self, user_id: &str, room_id: &str) 
            -> Result<Option<Membership>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT membership
                FROM room_memberships
                WHERE user_id = $1 AND room_id = $2;
        ", &[&user_id, &room_id]).await?;
        match rows.get(0) {
            Some(row) => {
                let membership_str: &str = row.get("membership");
                let membership = membership_str.parse().unwrap();
                Ok(Some(membership))
            },
            None => Ok(None),
        }
    }

    async fn get_room_member_counts(&mut self, room_id: &str) -> Result<(i64, i64), DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT membership, COUNT(membership) FROM room_memberships
                WHERE room_id = $1
                GROUP BY membership;
        ", &[&room_id]).await?;
        let mut joined_member_count = 0;
        let mut invited_member_count = 0;
        for row in rows {
            let count = row.get("count");
            match row.get("membership") {
                "join" => joined_member_count = count,
                "invite" => invited_member_count = count,
                _ => {},
            }
        }
        Ok((joined_member_count, invited_member_count))
    }

    async fn get_full_state(&mut self, room_id: &str) -> Result<Vec<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT DISTINCT ON (type, state_key)
                sender, type, state_key, content, unsigned, redacts, hash, origin_server_ts
                FROM room_events
                WHERE room_id = $1 AND state_key != NULL
                ORDER BY type, state_key, depth DESC;
        ", &[&room_id]).await?;
        let mut ret = Vec::new();
        for row in rows {
            ret.push(Event {
                room_id: None,
                sender: row.get("sender"),
                ty: row.get("type"),
                state_key: row.get("state_key"),
                content: row.get("content"),
                unsigned: row.get("unsigned"),
                redacts: row.get("redacts"),
                event_id: row.get("hash"),
                origin_server_ts: row.get("origin_server_ts"),
            });
        }
        Ok(ret)
    }

    async fn get_state_event(&mut self, room_id: &str, event_type: &str, state_key: &str)
            -> Result<Option<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT DISTINCT ON (state_key)
                sender, type, state_key, content, unsigned, redacts, hash, origin_server_ts
                FROM room_events
                WHERE room_id = $1 AND type = $2 AND state_key = $3
                ORDER BY state_key, depth DESC;
        ", &[&room_id, &event_type, &state_key]).await?;
        if let Some(row) = rows.get(0) {
            Ok(Some(Event {
                room_id: None,
                sender: row.get("sender"),
                ty: row.get("type"),
                state_key: row.get("state_key"),
                content: row.get("content"),
                unsigned: row.get("unsigned"),
                redacts: row.get("redacts"),
                event_id: row.get("hash"),
                origin_server_ts: row.get("origin_server_ts"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn get_events_since(&mut self, room_id: &str, since: u64)
            -> Result<Vec<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let since: i64 = since as i64;
        let rows = db.query("
            SELECT
                sender, type, state_key, content, unsigned, redacts, hash, origin_server_ts
                FROM room_events
                WHERE room_id = $1 AND ordering > $2;
        ", &[&room_id, &since]).await?;
        let mut ret = Vec::new();
        for row in rows {
            ret.push(Event {
                room_id: None,
                sender: row.get("sender"),
                ty: row.get("type"),
                state_key: row.get("state_key"),
                content: row.get("content"),
                unsigned: row.get("unsigned"),
                redacts: row.get("redacts"),
                event_id: row.get("hash"),
                origin_server_ts: row.get("origin_server_ts"),
            });
        }
        Ok(ret)
    }

    async fn get_event(&mut self, room_id: &str, event_id: &str)
            -> Result<Option<Event>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT
                sender, type, state_key, content, unsigned, redacts, hash, origin_server_ts
                FROM room_events
                WHERE room_id = $1 AND event_id = $2;
        ", &[&room_id, &event_id]).await?;
        if let Some(row) = rows.get(0) {
            Ok(Some(Event {
                room_id: None,
                sender: row.get("sender"),
                ty: row.get("type"),
                state_key: row.get("state_key"),
                content: row.get("content"),
                unsigned: row.get("unsigned"),
                redacts: row.get("redacts"),
                event_id: row.get("hash"),
                origin_server_ts: row.get("origin_server_ts"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn get_prev_event_ids(&mut self, room_id: &str)
            -> Result<Option<(i64, Vec<String>)>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT depth, hash FROM room_events 
                WHERE depth = (SELECT MAX(depth) FROM room_events WHERE room_id = $1);
        ", &[&room_id]).await?;
        let mut ret = Vec::new();
        let mut depth = 0;
        for row in rows {
            ret.push(row.get("hash"));
            depth = row.get("depth");   // same every time but oh well
        }
        if ret.len() != 0 {
            return Ok(Some((depth, ret)));
        } else {
            return Ok(None);
        }
    }

    async fn get_user_account_data(&mut self, username: &str)
            -> Result<HashMap<String, JsonValue>, DbError> {
        let db = self.inner.as_mut().unwrap();
        let rows = db.query("
            SELECT (type, content) FROM user_account_data WHERE username = $1;
        ", &[&username]).await?;
        let mut ret = HashMap::new();
        for row in rows {
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
                ").await?;
                db.execute(&stmt,
                    &[&event.state_key, &event.room_id, &membership, &event.hashes.sha256]
                ).await?;
            } else {
                let stmt = db.prepare("
                    DELETE FROM room_memberships WHERE user_id = $1 AND room_id = $2;
                ").await?;
                db.execute(&stmt, &[&event.state_key, &event.room_id]).await?;
            }
        },
        "m.room.create" => {
            let stmt = db.prepare("
                INSERT INTO rooms(id)
                    VALUES ($1);
            ").await?;
            db.execute(&stmt, &[&event.room_id]).await?;
        }
        _ => {},
    }
    Ok(())
}
