use std::{collections::HashMap, convert::TryFrom};

use crate::{error::Error, events::{EventContent, PduV4, room::{Create, JoinRule, JoinRules, Member, Membership, PowerLevels}}, state::State, storage::Storage, util::MatrixId};

pub async fn auth_check_v1(db: &dyn Storage, pdu: &PduV4, state: &State) -> Result<bool, Error> {
    // This function panics a lot eg when auth_events or prev_events don't exist. This is
    // intentional at the moment because if we're crafting a new event and we get that stuff
    // wrong it's a program error, and I think if we're receiving an event via federation then
    // we should have already attempted to receive any missing {auth,prev}_events
    if let EventContent::Create(_) = pdu.event_content {
        if !pdu.prev_events.is_empty() {
            return Ok(false);
        }
        let room_id_domain = pdu.room_id.split_once(':').expect("invalid room id").1;
        if pdu.sender.domain() != room_id_domain {
            return Ok(false);
        }
        // cant check room version if v4 is embedded in the type system lmao
        return Ok(true);
    }

    let mut auth_events = HashMap::new();
    for event_id in pdu.auth_events.iter() {
        let pdu = db.get_pdu(&pdu.room_id, event_id).await?.expect("auth event doesn't exist");
        auth_events.insert((pdu.event_content.get_type().to_string(), pdu.state_key.clone().expect("auth event isn't state")), pdu);
    }

    if !auth_events.contains_key(&("m.room.create".to_string(), "".to_string())) {
        return Ok(false);
    }

    if pdu.event_content.get_type() == "m.room.aliases" {
        if pdu.state_key == None {
            return Ok(false);
        }
        // whee im ignoring step 4-2 because i cant find proper docs for it and it's probably
        // obsolete by now anywayyyyyyyyy
        // also 4-3 is misleading because it looks like a short circuit but isnt wheeeeeee
    }

    let creator = state.get_content::<Create>(db, "").await?.unwrap().creator;
    let power_levels = state.get_content::<PowerLevels>(db, "").await?
        .unwrap_or_else(|| PowerLevels::no_event_default_levels(&creator));

    if let EventContent::Member(content) = &pdu.event_content {
        match content.membership {
            Membership::Join => {
                // do step 5-2-2 before 5-2-1 because it makes more sense
                // users can't set other users' membership to join
                if pdu.state_key.as_deref() != Some(pdu.sender.as_str()) {
                    return Ok(false);
                }

                // if the room has just been created by this user, allow them to join
                if pdu.prev_events.len() == 1 {
                    let prev_event = db.get_pdu(&pdu.room_id, &pdu.prev_events[0]).await?
                        .expect("prev_event doesn't exist");
                    if let EventContent::Create(create_content) = prev_event.event_content {
                        if pdu.sender == create_content.creator {
                            return Ok(true);
                        }
                    } else {
                        // oh no
                        panic!("oh no");
                    }
                    // not so sure about this bit
                    return Ok(false);
                }

                // get the user's membership in this room if they have one
                let membership = state.get_content::<Member>(db, pdu.sender.as_str()).await?
                    .map(|c| c.membership);

                // don't let banned users join
                if membership == Some(Membership::Ban) {
                    return Ok(false);
                }

                // get the room's join rules
                let join_rule = state.get_content::<JoinRules>(db, "").await?
                    .map(|c| c.join_rule);

                if join_rule == Some(JoinRule::Invite)
                    && (membership == Some(Membership::Join) || membership == Some(Membership::Invite)) {
                        return Ok(true);
                    } else if join_rule == Some(JoinRule::Public) {
                        return Ok(true);
                    }

                return Ok(false);
            },
            Membership::Invite => {
                //TODO: third party invites

                // get the sender's membership in this room if they have one
                let sender_membership = state.get_content::<Member>(db, pdu.sender.as_str()).await?
                    .map(|c| c.membership);

                // can't invite people if you're not in the room yourdb
                if sender_membership != Some(Membership::Join) {
                    return Ok(false);
                }

                // can't invite people if they're banned or already in
                let target_user_id = pdu.state_key.clone().expect("invitation has no target");
                let target_user_membership = state.get_content::<Member>(db, &target_user_id).await?
                    .map(|c| c.membership);
                match target_user_membership {
                    Some(Membership::Join | Membership::Ban) => return Ok(false),
                    _ => {},
                }

                // can't invite people if you don't have permission to do so
                if power_levels.get_user_level(&pdu.sender) >= power_levels.invite() {
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            },
            Membership::Leave => {
                let sender_membership = state.get_content::<Member>(db, pdu.sender.as_str()).await?
                    .map(|c| c.membership);

                // if a user is leaving of their own accord, only allow it if they were
                // previously in the room, or if they are declining an invite
                if pdu.state_key.as_deref() == Some(pdu.sender.as_str()) {
                    match sender_membership {
                        Some(Membership::Join | Membership::Invite) => return Ok(true),
                        _ => return Ok(false),
                    }
                }

                // can't kick if you're not a member
                if sender_membership != Some(Membership::Join) {
                    return Ok(false);
                }

                let target_user_id = pdu.state_key.clone().expect("kick has no target");
                let target_user_membership = state.get_content::<Member>(db, &target_user_id).await?
                    .map(|c| c.membership);

                // can't turn someone's ban to a kick if you don't have permission to unban
                if target_user_membership == Some(Membership::Ban)
                    && power_levels.get_user_level(&pdu.sender) < power_levels.ban() {
                        return Ok(false);
                    }

                // can only kick someone if you have permission to kick, and they're lower than
                // you in power level
                let sender_level = power_levels.get_user_level(&pdu.sender);
                let target_level = power_levels.get_user_level(
                    &MatrixId::try_from(target_user_id).expect("target not valid matrix id")
                    );
                if sender_level >= power_levels.kick() && sender_level > target_level {
                    return Ok(true);
                }

                return Ok(false);
            },
            Membership::Ban => {
                let sender_membership = state.get_content::<Member>(db, pdu.sender.as_str()).await?
                    .map(|c| c.membership);

                // can't ban someone if you're not a member
                if sender_membership != Some(Membership::Join) {
                    return Ok(false);
                }

                let sender_level = power_levels.get_user_level(&pdu.sender);
                let target_user_id = pdu.state_key.clone().expect("ban has no target");
                let target_level = power_levels.get_user_level(
                    &MatrixId::try_from(target_user_id).expect("target not valid matrix id")
                    );

                if sender_level >= power_levels.ban() && sender_level > target_level {
                    return Ok(true);
                }

                return Ok(false);
            }
            _ => return Ok(false),
        }
    }

    let sender_membership = state.get_content::<Member>(db, pdu.sender.as_str()).await?
        .map(|c| c.membership);

    if sender_membership != Some(Membership::Join) {
        return Ok(false);
    }

    let user_level = power_levels.get_user_level(&pdu.sender);

    if pdu.event_content.get_type() == "m.room.third_party_invite" {
        if power_levels.get_user_level(&pdu.sender) >= power_levels.invite() {
            return Ok(true);
        } else {
            return Ok(false);
        }
    }

    if user_level < power_levels.get_event_level(&pdu.event_content.get_type(), pdu.state_key.is_some()) {
        return Ok(false);
    }

    if let Some(state_key) = &pdu.state_key {
        if state_key.starts_with('@') && state_key != pdu.sender.as_str() {
            return Ok(false);
        }
    }

    if let EventContent::PowerLevels(new_power_levels) = &pdu.event_content {
        let old_power_levels = power_levels;
        // if there is no event then old_power_levels contains the effective power levels, so
        // we can't check via that and we have to hit the state map again
        if state.get(("m.room.power_levels", "")) == None {
            return Ok(true);
        }

        let sender_level = old_power_levels.get_user_level(&pdu.sender);

        if old_power_levels.ban() != new_power_levels.ban()
            && (old_power_levels.ban() > sender_level || new_power_levels.ban() > sender_level) {
                return Ok(false);
            }
        if old_power_levels.invite() != new_power_levels.invite()
            && (old_power_levels.invite() > sender_level || new_power_levels.invite() > sender_level) {
                return Ok(false);
            }
        if old_power_levels.kick() != new_power_levels.kick()
            && (old_power_levels.kick() > sender_level || new_power_levels.kick() > sender_level) {
                return Ok(false);
            }
        if old_power_levels.redact() != new_power_levels.redact()
            && (old_power_levels.redact() > sender_level || new_power_levels.redact() > sender_level) {
                return Ok(false);
            }
        if old_power_levels.events_default() != new_power_levels.events_default()
            && (old_power_levels.events_default() > sender_level || new_power_levels.events_default() > sender_level) {
                return Ok(false);
            }
        if old_power_levels.state_default() != new_power_levels.state_default()
            && (old_power_levels.state_default() > sender_level || new_power_levels.state_default() > sender_level) {
                return Ok(false);
            }
        if old_power_levels.users_default() != new_power_levels.users_default()
            && (old_power_levels.users_default() > sender_level || new_power_levels.users_default() > sender_level) {
                return Ok(false);
            }

        for (key, new_value) in new_power_levels.events.iter() {
            let old_value = old_power_levels.events.get(key);
            // if added or changed
            if old_value != Some(new_value) {
                if new_value > &sender_level {
                    return Ok(false);
                }
                // if there was an old value and it was greater than sender_level
                if old_value.map(|v| v > &sender_level) == Some(true) {
                    return Ok(false);
                }
            }
        }
        for (key, old_value) in old_power_levels.events.iter() {
            let new_value = new_power_levels.events.get(key);
            if new_value == None && old_value > &sender_level {
                return Ok(false);
            }
        }

        for (key, new_value) in new_power_levels.users.iter() {
            let old_value = old_power_levels.users.get(key);
            // if added or changed
            if old_value != Some(new_value) {
                if new_value > &sender_level {
                    return Ok(false);
                }
                // if there was an old value and it was greater than sender_level
                if old_value.map(|v| v > &sender_level) == Some(true) {
                    return Ok(false);
                }

                if old_value != None && key != &pdu.sender {
                    if old_value.unwrap() == &sender_level {
                        return Ok(false);
                    }
                }
            }
        }
        for (key, old_value) in old_power_levels.users.iter() {
            let new_value = new_power_levels.users.get(key);
            if new_value == None && old_value > &sender_level {
                return Ok(false);
            }
        }

        return Ok(true);
    }

    if let EventContent::Redaction(_) = pdu.event_content {
        let sender_level = power_levels.get_user_level(&pdu.sender);
        if sender_level >= power_levels.redact() {
            return Ok(true);
        }

        //TODO: figure out how to handle 11-2, given event id domains don't exist past room
        // version 4

        return Ok(false);
    }

    Ok(true)
}
