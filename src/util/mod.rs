use displaydoc::Display;
use pg::Error as DbError;

use crate::{
    events::{Event, UnhashedPdu, room},
    storage::postgres::ClientGuard,
};

#[derive(Display)]
pub enum AddEventsError {
    /// A user tried to send an event to a room which they are not in.
    UserNotInRoom,
    /// A user tried to join a room from which they are banned.
    UserBanned,
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
    /// One of the events to be added was invalid.
    InvalidEvent(String),

    /// A database error occurred.
    DbError(DbError),
}

impl From<DbError> for AddEventsError {
    fn from(e: DbError) -> Self {
        AddEventsError::DbError(e)
    }
}

#[async_trait::async_trait]
pub trait StorageExt {
    async fn add_events(
        &mut self,
        events: &[Event],
        room_id: &str,
    ) -> Result<(), AddEventsError>;
}

#[async_trait::async_trait]
impl StorageExt for ClientGuard {
    async fn add_events(
        &mut self,
        events: &[Event],
        room_id: &str,
    ) -> Result<(), AddEventsError> {
        let (power_levels, pl_event_id) = match self.get_state_event(room_id, "m.room.power_levels", "").await? {
            Some(v) => {
                (serde_json::from_value(v.content).map_err(AddEventsError::InvalidPowerLevels)?,
                Some(v.event_id.unwrap().clone()))
            },
            None => {
                let create_event = self.get_state_event(room_id, "m.room.create", "").await?
                    .ok_or(AddEventsError::RoomNotFound)?; //TODO: what if we're adding create?
                let create_content: room::Create = serde_json::from_value(create_event.content)
                    .map_err(AddEventsError::InvalidCreate)?;
                (room::PowerLevels::no_event_default_levels(&create_content.creator), None)
            },
        };
        // Validate events
        for event in events {
            match &*event.ty {
                "m.room.member" => {
                    validate_member_event(self, &event, room_id, &power_levels).await?;
                },
                _ => {
                    let sender_membership = self.get_membership(&event.sender, room_id).await?;
                    if sender_membership != Some(room::Membership::Join) {
                        return Err(AddEventsError::UserNotInRoom);
                    }
                    let user_level = power_levels.get_user_level(&event.sender);
                    let event_level = power_levels.get_event_level(
                        &event.ty,
                        event.state_key.is_some(),
                    );
                    if user_level < event_level {
                        return Err(AddEventsError::InsufficientPowerLevel);
                    }
                },
            }
        }

        let now = chrono::Utc::now().timestamp_millis();
        let (mut depth, mut prev_events) = self.get_prev_event_ids(room_id).await?
            .ok_or(AddEventsError::RoomNotFound)?;
        let auth_events: Vec<String> = match pl_event_id {
            Some(v) => vec![v],
            None => vec![],
        };
        events.iter().map(|event| {
            let pdu = UnhashedPdu {
                room_id: room_id.to_string(),
                sender: event.sender,
                ty: event.ty,
                state_key: event.state_key,
                content: event.content,
                unsigned: event.unsigned,
                redacts: event.redacts,
                origin: event.sender.split(':').nth(1).unwrap().to_string(),
                origin_server_ts: now,
                prev_events,
                depth,
                auth_events: auth_events.clone(),
            }.finalize();
            depth += 1;
            prev_events = vec![pdu.hashes.sha256.clone()];
        });
        Ok(())
    }
}

async fn validate_member_event(
    db: &mut ClientGuard,
    event: &Event,
    room_id: &str,
    power_levels: &room::PowerLevels,
) -> Result<(), AddEventsError> {
    let sender_membership = db.get_membership(&event.sender, room_id).await?;
    let affected_user = event.state_key.ok_or_else(
        || AddEventsError::InvalidEvent(format!("no state key in m.room.member event"))
    )?;
    let prev_membership = db.get_membership(&affected_user, room_id).await?;
    let new_member_content: room::Member = serde_json::from_value(event.content)
        .map_err(|e| AddEventsError::InvalidEvent(format!("{}", e)))?;
    let new_membership = new_member_content.membership;
    use room::Membership::*;
    match new_membership {
        Join => {
            if affected_user != event.sender {
                return Err(AddEventsError::InvalidEvent(
                    "user tried to set someone else's membership to join".to_string()
                ));
            }
            match prev_membership {
                Some(Join) | Some(Invite) => {},
                Some(Ban) => return Err(AddEventsError::UserBanned),
                _ => {
                    let join_rules_event = db.get_state_event(room_id, "m.room.join_rules", "").await?;
                    let is_public = match join_rules_event {
                        Some(e) => {
                            let join_rules: room::JoinRules = serde_json::from_value(e.content)
                                .map_err(AddEventsError::InvalidJoinRules)?;
                            join_rules.join_rule == room::JoinRule::Public
                        },
                        None => false,
                    };
                },
            }
        },
        Leave => {
            if sender_membership != Some(room::Membership::Join) {
                return Err(AddEventsError::UserNotInRoom);
            }
            if event.state_key != Some(event.sender) {
                // users can set own membership to leave, but setting others'
                // to leave is kicking and you need permission for that
                let user_level = power_levels.get_user_level(&event.sender);
                let kick_level = power_levels.kick();
                if user_level < kick_level {
                    return Err(AddEventsError::InsufficientPowerLevel);
                }
            }
        },
        Ban => {
            if sender_membership != Some(room::Membership::Join) {
                return Err(AddEventsError::UserNotInRoom);
            }
            let user_level = power_levels.get_user_level(&event.sender);
            let ban_level = power_levels.kick();
            if user_level < ban_level {
                return Err(AddEventsError::InsufficientPowerLevel);
            }
        },
        Invite => {
            if sender_membership != Some(room::Membership::Join) {
                return Err(AddEventsError::UserNotInRoom);
            }
            let user_level = power_levels.get_user_level(&event.sender);
            let invite_level = power_levels.invite();
            if user_level < invite_level {
                return Err(AddEventsError::InsufficientPowerLevel);
            }
        },
        Knock => unimplemented!(),
    }
    Ok(())
}
