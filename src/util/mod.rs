use actix_web::{post, web::{Data, Json}};
use async_trait::async_trait;
use displaydoc::Display;
use serde_json::Value as JsonValue;
use std::{
    error::Error,
    sync::Arc,
};

use crate::{
    events::{Event, UnhashedPdu, room},
    storage::{Storage, StorageManager},
    ServerState,
};

type DbError = <crate::StorageManager as StorageManager>::Error;

#[derive(Debug, Display)]
pub enum AddEventError {
    /// A user tried to send an event to a room which they are not in.
    UserNotInRoom,
    /// A user tried to join a room from which they are banned.
    UserBanned,
    /// A user tried to join a private room to which they were not invited.
    UserNotInvited,
    /// A user tried to send an event to a room which does not exist.
    RoomNotFound,
    /// The latest `m.room.power_levels` event in this room is invalid.
    InvalidPowerLevels(serde_json::Error),
    /// The latest `m.room.create` event in this room is invalid.
    InvalidCreate(serde_json::Error),
    /// The latest `m.room.join_rules` event in this room is invalid.
    InvalidJoinRules(serde_json::Error),
    /// The user does not have the required power level to send this event.
    InsufficientPowerLevel,
    /// The event to be added was invalid.
    InvalidEvent(String),

    /// A database error occurred.
    DbError(DbError),
}

impl From<DbError> for AddEventError {
    fn from(e: DbError) -> Self {
        AddEventError::DbError(e)
    }
}

#[async_trait]
pub trait StorageExt {
    async fn add_event(
        &mut self,
        event: Event,
        room_id: &str,
    ) -> Result<String, AddEventError>;

    async fn create_test_users(&mut self) -> Result<(), DbError>;
}

#[async_trait]
impl<T: Storage<Error = DbError>> StorageExt for T {
    async fn add_event(
        &mut self,
        event: Event,
        room_id: &str,
    ) -> Result<String, AddEventError> {
        let (power_levels, pl_event_id) = match self.get_state_event(room_id, "m.room.power_levels", "").await? {
            Some(v) => {
                (serde_json::from_value(v.content).map_err(AddEventError::InvalidPowerLevels)?,
                Some(v.event_id.unwrap().clone()))
            },
            None => {
                let create_event = self.get_state_event(room_id, "m.room.create", "").await?
                    .ok_or(AddEventError::RoomNotFound)?; //TODO: what if we're adding create?
                let create_content: room::Create = serde_json::from_value(create_event.content)
                    .map_err(AddEventError::InvalidCreate)?;
                (room::PowerLevels::no_event_default_levels(&create_content.creator), None)
            },
        };
        // Validate event
        match &*event.ty {
            "m.room.member" => {
                validate_member_event(self, &event, room_id, &power_levels).await?;
            },
            _ => {
                let sender_membership = self.get_membership(&event.sender, room_id).await?;
                if sender_membership != Some(room::Membership::Join) {
                    return Err(AddEventError::UserNotInRoom);
                }
                let user_level = power_levels.get_user_level(&event.sender);
                let event_level = power_levels.get_event_level(
                    &event.ty,
                    event.state_key.is_some(),
                );
                if user_level < event_level {
                    return Err(AddEventError::InsufficientPowerLevel);
                }
            },
        }

        let now = chrono::Utc::now().timestamp_millis();
        let (depth, prev_events) = self.get_prev_event_ids(room_id).await?
            .ok_or(AddEventError::RoomNotFound)?;
        let auth_events: Vec<String> = match pl_event_id {
            Some(v) => vec![v],
            None => vec![],
        };
        let pdu = UnhashedPdu {
            room_id: room_id.to_string(),
            sender: event.sender.clone(),
            ty: event.ty,
            state_key: event.state_key,
            content: event.content,
            unsigned: event.unsigned,
            redacts: event.redacts,
            origin: event.sender.split(':').nth(1).unwrap().to_string(),
            origin_server_ts: now,
            prev_events,
            depth: depth + 1,
            auth_events: auth_events.clone(),
        }.finalize();
        let event_id = pdu.hashes.sha256.clone();

        self.add_pdus(&[pdu]).await?;
        Ok(event_id)
    }

    async fn create_test_users(&mut self) -> Result<(), DbError> {
        // all passwords are "password"
        self.create_user("alice", Some(
            "$argon2i$v=19$m=4096,t=3,p=1$c2FsdHNhbHQ$llvUdqp69y2RB629dCuG42kR5y+Occ/ziKV5kn3rSOM"
        )).await?;
        self.create_user("bob", Some(
            "$argon2i$v=19$m=4096,t=3,p=1$c2FsdHNhbHQ$llvUdqp69y2RB629dCuG42kR5y+Occ/ziKV5kn3rSOM"
        )).await?;
        self.create_user("carol", Some(
            "$argon2i$v=19$m=4096,t=3,p=1$c2FsdHNhbHQ$llvUdqp69y2RB629dCuG42kR5y+Occ/ziKV5kn3rSOM"
        )).await?;
        Ok(())
    }
}

async fn validate_member_event<S: Storage<Error = DbError>>(
    db: &mut S,
    event: &Event,
    room_id: &str,
    power_levels: &room::PowerLevels,
) -> Result<(), AddEventError> {
    let sender_membership = db.get_membership(&event.sender, room_id).await?;
    let affected_user = event.state_key.clone().ok_or_else(
        || AddEventError::InvalidEvent(format!("no state key in m.room.member event"))
    )?;
    let prev_membership = db.get_membership(&affected_user, room_id).await?;
    let new_member_content: room::Member = serde_json::from_value(event.content.clone())
        .map_err(|e| AddEventError::InvalidEvent(format!("{}", e)))?;
    let new_membership = new_member_content.membership;
    use room::Membership::*;
    match new_membership {
        Join => {
            if affected_user != event.sender {
                return Err(AddEventError::InvalidEvent(
                    "user tried to set someone else's membership to join".to_string()
                ));
            }
            match prev_membership {
                Some(Join) | Some(Invite) => {},
                Some(Ban) => return Err(AddEventError::UserBanned),
                _ => {
                    let join_rules_event = db.get_state_event(room_id, "m.room.join_rules", "").await?;
                    let is_public = match join_rules_event {
                        Some(e) => {
                            let join_rules: room::JoinRules = serde_json::from_value(e.content)
                                .map_err(AddEventError::InvalidJoinRules)?;
                            join_rules.join_rule == room::JoinRule::Public
                        },
                        None => false,
                    };
                    if !is_public {
                        return Err(AddEventError::UserNotInvited);
                    }
                },
            }
        },
        Leave => {
            if sender_membership != Some(room::Membership::Join) {
                return Err(AddEventError::UserNotInRoom);
            }
            if event.state_key.as_ref() != Some(&event.sender) {
                // users can set own membership to leave, but setting others'
                // to leave is kicking and you need permission for that
                let user_level = power_levels.get_user_level(&event.sender);
                let kick_level = power_levels.kick();
                if user_level < kick_level {
                    return Err(AddEventError::InsufficientPowerLevel);
                }
            }
        },
        Ban => {
            if sender_membership != Some(room::Membership::Join) {
                return Err(AddEventError::UserNotInRoom);
            }
            let user_level = power_levels.get_user_level(&event.sender);
            let ban_level = power_levels.ban();
            if user_level < ban_level {
                return Err(AddEventError::InsufficientPowerLevel);
            }
        },
        Invite => {
            if sender_membership != Some(room::Membership::Join) {
                return Err(AddEventError::UserNotInRoom);
            }
            let user_level = power_levels.get_user_level(&event.sender);
            let invite_level = power_levels.invite();
            if user_level < invite_level {
                return Err(AddEventError::InsufficientPowerLevel);
            }
        },
        Knock => unimplemented!(),
    }
    Ok(())
}

#[post("/_debug/print_the_world")]
pub async fn print_the_world(state: Data<Arc<ServerState>>) -> String {
    let mut db = state.db_pool.get_handle().await.unwrap();
    db.print_the_world().await.unwrap();
    String::new()
}
