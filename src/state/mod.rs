use std::{borrow::Cow, cmp::Ordering, collections::{HashMap, HashSet}, convert::TryInto, sync::{Arc, Mutex}};

use crate::{error::Error, events::{EventContent, EventType, PduV4, room::{Member, Membership}}, storage::Storage};

use super::StorageExt;

/// (event_type, state_key) -> event_id
#[derive(Clone)]
pub struct State {
    room_id: String,
    map: HashMap<(Cow<'static, str>, Cow<'static, str>), String>,
}

impl State {
    pub fn get<'s, 'k: 's>(&'s self, (event_type, state_key): (&'k str, &'k str)) -> Option<&'s str> {
        let key = (Cow::from(event_type), Cow::from(state_key));
        self.map.get::<(Cow<'k, str>, Cow<'k, str>)>(&key).map(String::as_str)
    }

    pub async fn get_content<T: EventType>(&self, db: &dyn Storage, state_key: &str) -> Result<Option<T>, Error> {
        if let Some(event_id) = self.get((T::EVENT_TYPE, state_key)) {
            let event = db.get_pdu(&self.room_id, &event_id).await?
                .expect("event in state doesn't exist");
            return Ok(Some(event.event_content.try_into().map_err(|_| ()).unwrap()))
        }
        Ok(None)
    }

    pub fn insert_event(&mut self, pdu: &PduV4) {
        self.map.insert(
            (Cow::from(pdu.event_content.get_type().to_string()), Cow::from(pdu.state_key.clone().unwrap())),
            pdu.event_id().clone()
        );
    }
}

pub struct StateResolver {
    /// event_id -> state after that event S'(E)
    cache: Arc<Mutex<HashMap<String, State>>>,
    db: Box<dyn Storage>,
}

impl StateResolver {
    pub fn new(db: Box<dyn Storage>) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            db,
        }
    }

    pub async fn resolve(&self, room_id: &str, event: &PduV4) -> Result<State, Error> {
        self.resolve_v2(room_id, event).await
    }

    #[async_recursion::async_recursion]
    pub async fn resolve_v2(&self, room_id: &str, event: &PduV4) -> Result<State, Error> {
        let prev_event_ids = &event.prev_events;

        // event_id -> state
        let mut scratch = HashMap::new();

        // get as many entries from the cache as possible
        {
            let cache = self.cache.lock().unwrap();
            for event_id in prev_event_ids {
                if let Some(state) = cache.get(event_id) {
                    scratch.insert(event_id.to_string(), state.clone());
                }
            }
        }

        // fill in the gaps - in a separate loop so we don't lock for long
        for event_id in prev_event_ids {
            if scratch.contains_key(event_id) {
                continue;
            }
            let prev_event = self.db.get_pdu(room_id, &event_id).await?.unwrap();
            let state = self.resolve_v2(room_id, &prev_event).await?;
            scratch.insert(event_id.clone(), state);
        }

        // STEP 1

        // (event_type, state_key) -> [event_id]
        // used to construct the conflicted state set, where there is more than one event_id per
        // (event_type, state_key)
        let mut state_set = HashMap::new();
        for state in scratch.values() {
            for (type_and_key, event_id) in state.map.iter() {
                if !state_set.contains_key(type_and_key) {
                    state_set.insert((type_and_key.0.clone(), type_and_key.1.clone()), HashSet::new());
                }
                state_set.get_mut(type_and_key).unwrap().insert(event_id.clone());
            }
        }
        let mut unconflicted_state_map = HashMap::new();
        let mut conflicted_state_set = HashSet::new();
        for (type_and_key, event_ids) in state_set.into_iter() {
            if event_ids.len() == 1 {
                let event_id = event_ids.into_iter().next().unwrap();
                unconflicted_state_map.insert(type_and_key, event_id);
            } else {
                conflicted_state_set.extend(event_ids);
            }
        }

        let auth_difference = self.auth_difference(room_id, prev_event_ids).await?;
        let mut full_conflicted_set = conflicted_state_set.union(&auth_difference).map(Clone::clone).collect::<HashSet<_>>();

        // The spec says we're also supposed to take all the events from these events' auth chains
        // that are in the full conflicted set, but Synapse doesn't seem to do anything special so
        // either it's not important, they get implicitly taken due to some subtle relationship
        // that I don't understand, or Synapse is broken.
        // Or I can't read
        let mut power_events = HashSet::new();
        for event_id in full_conflicted_set.iter() {
            let event = self.db.get_pdu(room_id, event_id).await?.unwrap();
            if is_power_event(&event) {
                power_events.insert(event_id.clone());
            }
        }

        for event_id in power_events.iter() {
            full_conflicted_set.remove(&**event_id);
        }

        let events_list = self.reverse_topological_power_ordering(room_id, power_events).await?;

        // STEP 2
        // where the auth checks happen ???

        let mut partially_resolved_state = unconflicted_state_map.clone();

        for event_id in events_list {
            let event = self.db.get_pdu(room_id, &event_id).await?.expect("event not found");
            if let Some(state_key) = &event.state_key {
                let auth_event_keys_needed = auth_types_for_event(&event);
                let mut frankenstate = State {
                    room_id: String::from(room_id),
                    map: HashMap::new(),
                };
                for auth_event_id in event.auth_events.iter() {
                    let auth_event = self.db.get_pdu(room_id, auth_event_id).await?.unwrap();
                    // TODO: check whether it is rejected
                    frankenstate.insert_event(&auth_event);
                }
                frankenstate.map.extend(partially_resolved_state.clone().into_iter());
                crate::validate::auth::auth_check_v1(&*self.db, &event, &frankenstate).await?;
                partially_resolved_state.insert((Cow::from(event.event_content.get_type().to_string()), Cow::from(state_key.clone())), event_id.clone());
            }
        }

        // STEP 3
        // mainline ordering D:

        let get_power_levels = |event: PduV4| async move {
            for auth_event_id in event.auth_events.iter() {
                let auth_event = self.db.get_pdu(room_id, auth_event_id).await?.unwrap();
                match auth_event.event_content {
                    EventContent::PowerLevels(_) =>
                        return Result::<Option<String>, Error>::Ok(Some(auth_event_id.clone())),
                    _ => continue,
                }
            }
            // probably shouldn't happen
            Result::<Option<String>, Error>::Ok(None)
        };

        // i seriously have no idea what to do if this doesn't exist
        let mainline_starting_point = partially_resolved_state
            .get(&(Cow::from("m.room.power_levels"), Cow::from(""))).expect("oh no");
        let mut mainline = vec![mainline_starting_point.clone()];
        let mut current = mainline_starting_point.clone();
        while let Some(parent) = get_power_levels(self.db.get_pdu(room_id, &current).await?.unwrap()).await? {
            mainline.push(parent.clone());
            current = parent;
        }

        // Tuple of event_id and index of closest mainline event to that event
        let mut events_with_closest_mainlines = Vec::new();
        for event_id in full_conflicted_set.iter() {
            let mut current = event_id.clone();
            let closest_mainline = 'inner: loop {
                if let Some((index, _)) = mainline.iter().enumerate().find(|(_, id)| **id == current) {
                    break 'inner index;
                }

                let current_event = self.db.get_pdu(room_id, &current).await?.unwrap();
                match get_power_levels(current_event).await? {
                    Some(id) => current = id.clone(),
                    None => break 'inner std::usize::MAX,
                }
            };

            let event = self.db.get_pdu(room_id, event_id).await?.unwrap();
            events_with_closest_mainlines.push((event, closest_mainline));
        }

        events_with_closest_mainlines.sort_by(mainline_cmp);

        // STEP 4
        // iterative auth checks again

        for (event, _) in events_with_closest_mainlines.into_iter() {
            if let Some(state_key) = &event.state_key {
                let auth_event_keys_needed = auth_types_for_event(&event);
                let mut frankenstate = State {
                    room_id: String::from(room_id),
                    map: HashMap::new(),
                };
                for auth_event_id in event.auth_events.iter() {
                    let auth_event = self.db.get_pdu(room_id, &auth_event_id).await?.unwrap();
                    // TODO: check whether it is rejected
                    frankenstate.insert_event(&auth_event);
                }
                frankenstate.map.extend(partially_resolved_state.clone().into_iter());
                crate::validate::auth::auth_check_v1(&*self.db, &event, &frankenstate).await?;
                partially_resolved_state.insert((Cow::from(event.event_content.get_type().to_string()), Cow::from(state_key.clone())), event.event_id().clone());
            }
        }

        // STEP 5 ???????

        for (type_and_key, event_id) in unconflicted_state_map.into_iter() {
            partially_resolved_state.insert(type_and_key, event_id);
        }

        Ok(State {
            room_id: room_id.to_string(),
            map: partially_resolved_state,  // not partially anymore lmao
        })
    }

    async fn auth_chains(&self, room_id: &str, event_ids: &[String]) -> Result<HashSet<String>, Error> {
        let mut ret = HashSet::new();
        let mut to_check = Vec::new();
        to_check.extend_from_slice(event_ids);
        while !to_check.is_empty() {
            let event_id = to_check.pop().unwrap();
            let pdu = self.db.get_pdu(room_id, &event_id).await?.expect("event not found");
            for auth_event_id in pdu.auth_events {
                if !ret.contains(&auth_event_id) {
                    to_check.push(auth_event_id.clone());
                }
                ret.insert(auth_event_id);
            }
        }

        Ok(ret)
    }

    async fn auth_difference(&self, room_id: &str, event_ids: &[String]) -> Result<HashSet<String>, Error> {
        assert_ne!(event_ids.len(), 0);
        if event_ids.len() == 1 {
            return Ok(HashSet::new())
        }
        let mut chains = Vec::new();
        for event_id in event_ids {
            chains.push(self.auth_chains(room_id, &[event_id.clone()]).await?);
        }
        // these unwraps are gucci because we already panicked at the start
        let intersection = {
            let mut iter = chains.iter();
            let first = iter.next().unwrap().clone();
            iter.fold(
                first,
                |acc, x| acc.intersection(&x).cloned().collect()
                )
        };
        let union = chains.iter().fold(HashSet::new(), |acc, x| acc.union(&x).cloned().collect());
        let difference = union.difference(&intersection).cloned().collect();
        Ok(difference)
    }

    async fn reverse_topological_power_ordering(&self, room_id: &str, event_ids: HashSet<String>) -> Result<Vec<String>, Error> {
        let mut events = HashMap::new();
        for event_id in event_ids {
            let event = self.db.get_pdu(room_id, &event_id).await?.expect("event not found");
            events.insert(event_id, event);
        }

        let mut ret = Vec::new();
        let mut candidates = Vec::new();
        while events.len() > 0 {
            // get the events in the graph with no parents
            'outer: for (id1, event1) in events.iter() {
                for (_id2, event2) in events.iter() {
                    if event2.auth_events.contains(&id1) {
                        continue 'outer;
                    }
                }
                candidates.push(event1);
            }

            let mut sender_power_levels = HashMap::new();
            for event in candidates.iter() {
                let power_level = self.db.get_sender_power_level(&event.room_id, &event.event_id()).await?;
                sender_power_levels.insert(event.event_id(), power_level);
            }

            candidates.sort_by(|a, b| {
                let a_power_level = sender_power_levels.get(&a.event_id());
                let b_power_level = sender_power_levels.get(&b.event_id());
                let power_level_ordering = a_power_level.cmp(&b_power_level);
                if power_level_ordering != Ordering::Equal {
                    return power_level_ordering;
                }

                let ts_ordering = a.origin_server_ts.cmp(&b.origin_server_ts);
                if ts_ordering != Ordering::Equal {
                    return ts_ordering;
                }

                // aaaaaaaaa
                return a.event_id().cmp(&b.event_id());
            });

            ret.extend(candidates.drain(..).map(|pdu| pdu.event_id()));
        }

        Ok(ret)
    }
}

fn is_power_event(pdu: &PduV4) -> bool {
    match pdu.event_content {
        EventContent::PowerLevels(_) | EventContent::JoinRules(_) => true,
        EventContent::Member(Member { membership: Membership::Leave, .. }) |
            EventContent::Member(Member { membership: Membership::Ban, .. })
            if pdu.state_key.as_deref() != Some(pdu.sender.as_str()) => true,
        _ => false,
    }
}

fn auth_types_for_event(pdu: &PduV4) -> HashSet<(&str, &str)> {
    let mut ret = HashSet::new();
    if pdu.event_content.get_type() == "m.room.create" {
        return ret;
    }

    ret.insert(("m.room.create", ""));
    ret.insert(("m.room.member", &pdu.sender.as_str()));
    ret.insert(("m.room.power_levels", ""));

    if let EventContent::Member(ref member) = pdu.event_content {
        ret.insert(("m.room.member", pdu.state_key.as_ref().unwrap()));

        let membership = &member.membership;
        if *membership == Membership::Join || *membership == Membership::Invite {
            ret.insert(("m.room.join_rules", ""));
        }

        // TODO: third party invites. see synapse event_auth.py
    }

    ret
}

fn mainline_cmp(x: &(PduV4, usize), y: &(PduV4, usize)) -> Ordering {
    // list is sorted backwards
    let mainline_based_order = x.1.cmp(&y.1).reverse();
    if mainline_based_order.is_ne() {
        return mainline_based_order;
    }

    // time, however, is not
    let ts_based_order = x.0.origin_server_ts.cmp(&y.0.origin_server_ts);
    if ts_based_order.is_ne() {
        return ts_based_order;
    }

    let id_based_order = x.0.event_id().cmp(&y.0.event_id());
    if id_based_order.is_ne() {
        return id_based_order;
    }

    panic!("oh come on now");
}
