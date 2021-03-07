use async_trait::async_trait;
use displaydoc::Display;
use enum_extract::extract;
use std::convert::TryInto;

use crate::{
    events::{Event, EventContent, room},
    storage::{Storage, StorageManager},
    util::MxidError,
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
    ) -> Result<String, AddEventError>;

    async fn create_test_users(&mut self) -> Result<(), DbError>;
}

#[async_trait]
impl<T: Storage<Error = DbError>> StorageExt for T {
    async fn add_event(
        &mut self,
        event: Event,
    ) -> Result<String, AddEventError> {
        let room_id = event.room_id.as_ref().unwrap();
        let (power_levels, pl_event_id) = match self.get_state_event(room_id, "m.room.power_levels", "").await? {
            Some(v) => (
                extract!(EventContent::PowerLevels(_), v.event_content).unwrap(),
                Some(v.event_id.unwrap().clone()),
            ),
            None => {
                let create_event = self.get_state_event(room_id, "m.room.create", "").await?
                    .ok_or(AddEventError::RoomNotFound)?; //TODO: what if we're adding create?
                let create_content =
                    extract!(EventContent::Create(_), create_event.event_content).unwrap();
                (room::PowerLevels::no_event_default_levels(&create_content.creator), None)
            },
        };
        // Validate event
        match event.event_content {
            EventContent::Member(_) => {
                validate_member_event(self, &event, room_id, &power_levels).await?;
            },
            _ => {
                let sender_membership = self.get_membership(&event.sender, room_id).await?;
                if sender_membership != Some(room::Membership::Join) {
                    return Err(AddEventError::UserNotInRoom);
                }
                let user_level = power_levels.get_user_level(&event.sender);
                let event_level = power_levels.get_event_level(
                    &event.event_content.get_type(),
                    event.state_key.is_some(),
                );
                if user_level < event_level {
                    return Err(AddEventError::InsufficientPowerLevel);
                }
            },
        }

        let mut auth_events = Vec::with_capacity(1);
        match pl_event_id {
            Some(v) => auth_events.push(v),
            None => {},
        };
        let event_id = self.add_event_unchecked(event, auth_events).await?;
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
        || AddEventError::InvalidEvent("no state key in m.room.member event".to_string())
    )?.try_into().map_err(|e: MxidError| AddEventError::InvalidEvent(e.to_string()))?;
    let prev_membership = db.get_membership(&affected_user, room_id).await?;

    // can't use extract because it's behind a reference how sad is that
    let new_member_content = match event.event_content {
        EventContent::Member(ref v) => v,
        _ => panic!("m.room.member not a member event"),
    };
    let new_membership = &new_member_content.membership;
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
                            let join_rules =
                                extract!(EventContent::JoinRules(_), e.event_content).unwrap();
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
            if event.state_key.as_deref() != Some(event.sender.as_str()) {
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
