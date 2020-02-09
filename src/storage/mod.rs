use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use uuid::Uuid;

use crate::events::{room::Membership, Event, PduV4};

pub mod mem;
pub mod postgres;

#[derive(Clone, Debug, Default)]
pub struct UserProfile {
    pub avatar_url: Option<String>,
    pub displayname: Option<String>,
}

#[async_trait]
pub trait StorageManager {
    type Handle: Storage;
    type Error: std::error::Error;

    async fn get_handle(&self) -> Result<Self::Handle, Self::Error>;
}

#[async_trait]
pub trait Storage: Send {
    type Error: std::error::Error;

    async fn create_user(
        &mut self,
        username: &str,
        password_hash: Option<&str>, //TODO: accept "password" and do the hashing here
    ) -> Result<(), Self::Error>;

    async fn verify_password(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<bool, Self::Error>;

    async fn create_access_token(
        &mut self,
        username: &str,
        device_id: &str,
    ) -> Result<Uuid, Self::Error>;

    async fn delete_access_token(&mut self, token: Uuid) -> Result<(), Self::Error>;

    /// Deletes all access tokens associated with the same user as this one
    async fn delete_all_access_tokens(
        &mut self,
        token: Uuid,
    ) -> Result<(), Self::Error>;

    /// Returns the username for which this token is valid, if any
    async fn try_auth(&mut self, token: Uuid) -> Result<Option<String>, Self::Error>;

    /// Returns the given user's avatar URL and display name, if present
    async fn get_profile(&mut self, username: &str) -> Result<Option<UserProfile>, Self::Error>;

    async fn set_avatar_url(&mut self, username: &str, avatar_url: &str)
        -> Result<(), Self::Error>;

    async fn set_display_name(
        &mut self,
        username: &str,
        display_name: &str,
    ) -> Result<(), Self::Error>;

    async fn add_pdus(&mut self, pdus: &[PduV4]) -> Result<(), Self::Error>;

    async fn get_memberships_by_user(
        &mut self,
        user_id: &str,
    ) -> Result<HashMap<String, Membership>, Self::Error>;

    async fn get_membership(
        &mut self,
        user_id: &str,
        room_id: &str,
    ) -> Result<Option<Membership>, Self::Error>;

    /// Returns the number of users in a room and the number of users invited to the room.
    /// 
    /// Returns (0, 0) if the room does not exist.
    async fn get_room_member_counts(&mut self, room_id: &str) -> Result<(i64, i64), Self::Error>;

    async fn get_full_state(&mut self, room_id: &str) -> Result<Vec<Event>, Self::Error>;

    async fn get_state_event(
        &mut self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<Option<Event>, Self::Error>;

    async fn get_events_since(
        &mut self,
        room_id: &str,
        since: u64,
    ) -> Result<Vec<Event>, Self::Error>;

    async fn get_event(
        &mut self,
        room_id: &str,
        event_id: &str,
    ) -> Result<Option<Event>, Self::Error>;

    async fn get_prev_event_ids(
        &mut self,
        room_id: &str,
    ) -> Result<Option<(i64, Vec<String>)>, Self::Error>;

    async fn get_user_account_data(
        &mut self,
        username: &str,
    ) -> Result<HashMap<String, JsonValue>, Self::Error>;

    async fn print_the_world(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
