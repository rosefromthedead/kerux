use crossbeam::queue::ArrayQueue;
use futures::{
    compat::{Future01CompatExt, Stream01CompatExt},
    stream::{StreamExt, TryStreamExt},
};
use pg::{Client, Error as DbError, NoTls};
use serde_json::{Value as JsonValue, to_value as to_json_value};
use std::sync::Arc;

use crate::{
    client::error::Error,
    events::{room::Membership, RoomEventV4},
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
                tracing::warn!("{}", e); e
            })?;
            runtime::spawn(conn.compat());
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
    pub async fn create_user(&mut self, user_id: &str, password_hash: Option<&str>) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("INSERT INTO users(id, password_hash) VALUES ($1, $2);").compat().await?;
        db.execute(&stmt, &[&user_id, &password_hash]).compat().await?;
        Ok(())
    }

    pub async fn verify_password(&mut self, user_id: &str, password: &str) -> Result<bool, DbError> {
        let db = self.inner.as_mut().unwrap();
        let get_user = db.prepare("SELECT password_hash FROM users WHERE id=$1;").compat().await?;
        let mut rows = db.query(&get_user, &[&user_id]).compat(); 
        let user = match rows.next().await {
            Some(v) => v?,
            None => return Ok(false),
        };

        match argon2::verify_encoded(user.try_get("password_hash")?, password.as_bytes()) {
            Ok(true) => Ok(true),
            Ok(false) | Err(_) => Ok(false),
        }
    }

    pub async fn create_access_token(&mut self, user_id: &str, device_id: &str) -> Result<uuid::Uuid, DbError> {
        let db = self.inner.as_mut().unwrap();
        let access_token = uuid::Uuid::new_v4();
        let insert_token = db.prepare("INSERT INTO access_tokens(token, user_id, device_id) VALUES ($1, $2, $3);").compat().await?;
        db.execute(&insert_token, &[&access_token, &user_id, &device_id]).compat().await?;
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
            WITH id AS (
                SELECT user_id FROM access_tokens WHERE token = $1
            )
            DELETE FROM access_tokens WHERE user_id IN id;
        ").compat().await?;
        db.execute(&stmt, &[&token]).compat().await?;
        Ok(())
    }

    pub async fn try_auth(&mut self, token: uuid::Uuid) -> Result<String, Error> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("SELECT user_id FROM access_tokens WHERE token = $1;").compat().await?;
        let mut rows = db.query(&query, &[&token]).compat();
        let row = rows.next().await.ok_or(Error::UnknownToken)??;
        let id = row.try_get("user_id")?;
        Ok(id)
    }

    /// Returns the given user's avatar URL and display name, if present
    pub async fn get_profile(&mut self, user_id: &str) -> Result<(Option<String>, Option<String>), Error> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("SELECT avatar_url, display_name FROM users WHERE name = $1;").compat().await?;
        let mut rows = db.query(&query, &[&user_id]).compat();
        let row = match rows.next().await {
            Some(v) => v?,
            None => return Err(Error::NotFound),
        };
        let avatar_url: Option<String> = row.try_get("avatar_url")?;
        let display_name: Option<String> = row.try_get("display_name")?;
        Ok((avatar_url, display_name))
    }

    pub async fn set_avatar_url(&mut self, user_id: &str, avatar_url: &str) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("UPDATE users SET avatar_url = $1 WHERE id = $2;").compat().await?;
        db.execute(&stmt, &[&avatar_url, &user_id]).compat().await?;
        Ok(())
    }

    pub async fn set_display_name(&mut self, user_id: &str, display_name: &str) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        let stmt = db.prepare("UPDATE users SET display_name = $1 WHERE id = $2;").compat().await?;
        db.execute(&stmt, &[&display_name, &user_id]).compat().await?;
        Ok(())
    }

    pub async fn add_events(&mut self,
        events: impl IntoIterator<Item = crate::events::RoomEventV4>
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
                &JsonValue::from(event.content),
                &to_json_value(event.prev_events).unwrap(),
                &event.depth,
                &to_json_value(event.auth_events).unwrap(),
                &event.redacts,
                &event.unsigned.map(to_json_value).map(Result::unwrap),
                &event.hashes.sha256,
                &to_json_value(event.signatures).unwrap(),
            ]).compat().await?;
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
                let membership = match row?.get("membership") {
                   "ban" => Membership::Ban,
                   "invite" => Membership::Invite,
                   "join" => Membership::Join,
                   "knock" => Membership::Knock,
                   "leave" => Membership::Leave,
                };
                Ok((row?.get("room_id"), membership))
            })
            .try_collect::<Vec<(_, _)>>().await
    }

    pub async fn get_room_member_counts(&mut self, room_id: &str) -> Result<(i64, i64), DbError> {
        let db = self.inner.as_mut().unwrap();
        let query = db.prepare("
            SELECT membership, COUNT(membership) FROM room_memberships
                WHERE room_id = $1
                GROUP BY membership;
        ").compat().await?;
        let rows = db.query(&query, &[&room_id]).compat();
        let mut joined_member_count = 0;
        let mut invited_member_count = 0;
        while let Some(row) = rows.next().await {
            let count = row?.get("count");
            match row?.get("membership") {
                "join" => joined_member_count = count,
                "invite" => invited_member_count = count,
                _ => {},
            }
        }
        Ok((joined_member_count, invited_member_count))
    }
}

impl ClientGuard {
    async fn handle_event(&mut self, event: &RoomEventV4) -> Result<(), DbError> {
        let db = self.inner.as_mut().unwrap();
        match &*event.ty {
            "m.room.member" => {
                let membership = event.content.get("membership").unwrap();
                if membership != "leave" {
                    let stmt = db.prepare("
                        INSERT INTO room_memberships(user_id, room_id, membership, event_id)
                            VALUES ($1, $2, $3, $4)
                            ON CONFLICT DO UPDATE SET membership = $3, event_id = $4;
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
            _ => {},
        }
        Ok(())
    }
}
