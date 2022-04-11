use std::{borrow::Cow, cmp::Ordering, collections::{BTreeSet, HashMap, HashSet}, convert::TryInto, iter::FromIterator, sync::{Arc, Mutex}};

use futures::stream::{StreamExt, TryStreamExt};
use tracing::trace;

use crate::{error::Error, events::{EventContent, EventType, pdu::StoredPdu, room::{Member, Membership}, room_version::VersionedPdu}, storage::Storage, validate::auth::AuthStatus};

use super::StorageExt;

/// (event_type, state_key) -> event_id
#[derive(Clone)]
pub struct State {
    room_id: String,
    map: HashMap<(Cow<'static, str>, Cow<'static, str>), String>,
}

impl State {
    pub fn key<'k>((event_type, state_key): (&'k str, &'k str)) -> (Cow<'k, str>, Cow<'k, str>) {
        (Cow::from(event_type), Cow::from(state_key))
    }

    pub fn get<'s, 'k: 's>(&'s self, key_strs: (&'k str, &'k str)) -> Option<&'s str> {
        let key = Self::key(key_strs);
        self.map.get::<(Cow<'k, str>, Cow<'k, str>)>(&key).map(String::as_str)
    }

    pub async fn get_content<T: EventType>(&self, db: &dyn Storage, state_key: &str) -> Result<Option<T>, Error> {
        if let Some(event_id) = self.get((T::EVENT_TYPE, state_key)) {
            let event = db.get_pdu(&self.room_id, &event_id).await?
                .expect("event in state doesn't exist");
            return Ok(Some(event.event_content().clone().try_into().map_err(|_| ()).unwrap()))
        }
        Ok(None)
    }

    pub fn insert_event(&mut self, pdu: &VersionedPdu) {
        self.map.insert(
            (Cow::from(pdu.event_content().get_type().to_string()), Cow::from(pdu.state_key().unwrap().to_string())),
            pdu.event_id().to_string()
        );
    }
}

pub struct StateResolver {
    /// [event_id] -> state after those events res({S'(E1), S'(E2)})
    cache: Arc<Mutex<HashMap<BTreeSet<String>, State>>>,
    // TODO: do we want to keep this around, or pass it by function arguments?
    db: Box<dyn Storage>,
}

impl StateResolver {
    pub fn new(db: Box<dyn Storage>) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            db,
        }
    }

    pub async fn resolve(&self, room_id: &str, events: &[String]) -> Result<State, Error> {
        self.resolve_v2(room_id, events).await
    }

    #[tracing::instrument(level = tracing::Level::DEBUG, skip(self))]
    #[async_recursion::async_recursion]
    pub async fn resolve_v2(&self, room_id: &str, events: &[String]) -> Result<State, Error> {
        if events.len() == 0 {
            return Ok(State {
                room_id: room_id.to_owned(),
                map: HashMap::new(),
            });
        }

        let key = BTreeSet::from_iter(events.iter().map(ToOwned::to_owned));
        if let Some(state) = self.cache.lock().unwrap().get(&key) {
            trace!("state cache hit");
            return Ok(state.clone());
        }

        if events.len() == 1 {
            let event = self.db.get_pdu(room_id, &events[0]).await?.unwrap();
            let mut state = self.resolve_v2(room_id, event.prev_events()).await?;
            if event.did_pass_auth() && event.state_key().is_some() {
                trace!(
                    event_type=event.event_content().get_type(),
                    state_key=event.state_key().unwrap(),
                    "applying one event on top of state"
                );
                state.insert_event(&event.inner());
            }
            self.cache.lock().unwrap().insert(BTreeSet::from_iter([events[0].clone()]), state.clone());
            return Ok(state);
        }

        trace!("sad path");

        // event_id -> state
        let mut scratch = HashMap::new();

        // get as many entries from the cache as possible
        {
            let cache = self.cache.lock().unwrap();
            for event_id in events.iter() {
                if let Some(state) = cache.get(&BTreeSet::from_iter([event_id.clone()])) {
                    scratch.insert(event_id.to_string(), state.clone());
                }
            }
        }

        // fill in the gaps - in a separate loop so we don't lock for long
        for event_id in events.iter() {
            if scratch.contains_key(event_id) {
                continue;
            }
            let state = self.resolve_v2(room_id, &[event_id.clone()]).await?;
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

        let auth_difference = self.auth_difference(room_id, events).await?;
        let mut full_conflicted_set = conflicted_state_set.union(&auth_difference).map(Clone::clone).collect::<HashSet<_>>();

        // The spec says we're also supposed to take all the events from these events' auth chains
        // that are in the full conflicted set, but Synapse doesn't seem to do anything special so
        // either it's not important, they get implicitly taken due to some subtle relationship
        // that I don't understand, or Synapse is broken.
        // Or I can't read
        let mut power_events = HashSet::new();
        for event_id in full_conflicted_set.iter() {
            let event = self.db.get_pdu(room_id, event_id).await?.unwrap();
            if is_power_event(&event.inner()) {
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
            if let Some(state_key) = &event.state_key() {
                let auth_event_keys_needed = auth_types_for_event(&event.inner());
                let mut frankenstate = State {
                    room_id: String::from(room_id),
                    map: HashMap::new(),
                };
                for auth_event_id in event.auth_events().iter() {
                    let auth_event = self.db.get_pdu(room_id, auth_event_id).await?.unwrap();
                    if auth_event.did_pass_auth() {
                        frankenstate.insert_event(auth_event.inner());
                    }
                }
                frankenstate.map.extend(partially_resolved_state.clone().into_iter());
                crate::validate::auth::auth_check_v1(&*self.db, event.inner(), &frankenstate).await?;
                let state_key = String::from(*state_key);
                partially_resolved_state.insert((Cow::from(event.event_content().get_type().to_string()), Cow::from(state_key)), event_id.clone());
            }
        }

        // STEP 3
        // mainline ordering D:

        let get_power_levels = |event: VersionedPdu| async move {
            let auth_events = event.auth_events().clone();
            for auth_event_id in auth_events.iter() {
                let auth_event = self.db.get_pdu(room_id, auth_event_id).await?.unwrap();
                match auth_event.event_content() {
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
        while let Some(parent) = get_power_levels(self.db.get_pdu(room_id, &current).await?.unwrap().inner().clone()).await? {
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
                match get_power_levels(current_event.inner().clone()).await? {
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

        let new_state_events = events_with_closest_mainlines
            .iter()
            .map(|(e, _m)| &e.inner)
            .filter(|e| e.state_key().is_some());
        let mut partially_resolved_state = self.iterative_auth_checks(State { room_id: room_id.to_owned(), map: partially_resolved_state }, new_state_events).await?;

        // STEP 5 ???????

        for (type_and_key, event_id) in unconflicted_state_map.into_iter() {
            partially_resolved_state.map.insert(type_and_key, event_id);
        }

        Ok(partially_resolved_state) // not partially anymore lmao
    }

    async fn auth_chains(&self, room_id: &str, event_ids: &[String]) -> Result<HashSet<String>, Error> {
        let mut ret = HashSet::new();
        let mut to_check = Vec::new();
        to_check.extend_from_slice(event_ids);
        while !to_check.is_empty() {
            let event_id = to_check.pop().unwrap();
            let pdu = self.db.get_pdu(room_id, &event_id).await?.expect("event not found");
            for auth_event_id in pdu.auth_events() {
                if !ret.contains(auth_event_id) {
                    to_check.push(auth_event_id.clone());
                }
                ret.insert(auth_event_id.clone());
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
                    if event2.auth_events().contains(&id1) {
                        continue 'outer;
                    }
                }
                candidates.push(event1);
            }

            let mut sender_power_levels = HashMap::new();
            for event in candidates.iter() {
                let power_level = self.db.get_sender_power_level(&event.room_id(), &event.event_id()).await?;
                sender_power_levels.insert(event.event_id(), power_level);
            }

            candidates.sort_by(|a, b| {
                let a_power_level = sender_power_levels.get(&a.event_id());
                let b_power_level = sender_power_levels.get(&b.event_id());
                let power_level_ordering = a_power_level.cmp(&b_power_level);
                if power_level_ordering != Ordering::Equal {
                    return power_level_ordering;
                }

                let ts_ordering = a.origin_server_ts().cmp(&b.origin_server_ts());
                if ts_ordering != Ordering::Equal {
                    return ts_ordering;
                }

                // aaaaaaaaa
                return a.event_id().cmp(&b.event_id());
            });

            ret.extend(candidates.drain(..).map(|pdu| pdu.event_id().to_string()));
        }

        Ok(ret)
    }

    async fn iterative_auth_checks<'this, 'pdu>(
        &'this self,
        mut state: State,
        state_events: impl IntoIterator<Item = &'pdu VersionedPdu>,
    ) -> Result<State, Error> {
        for event in state_events {
            // fetch everything referenced in event.auth_events
            let future_iter = event
                .auth_events()
                .iter()
                .map(|event_id| self.db.get_pdu(&state.room_id, event_id));
            let auth_events = futures::stream::iter(future_iter)
                .then(|f| f)
                .try_filter_map(|opt| async { Ok(opt) })
                .try_collect::<Vec<_>>()
                .await?;

            // for auth checking, prefer events from state, otherwise fall back to auth_events
            let mut frankenstate = state.clone();
            for auth_key in auth_types_for_event(event) {
                if !frankenstate.map.contains_key(&State::key(auth_key)) {
                    let fallback_event = auth_events
                        .iter()
                        .find(|pdu| pdu.event_content().get_type() == auth_key.0 && pdu.state_key() == Some(auth_key.1));
                    if let Some(pdu) = fallback_event {
                        frankenstate.insert_event(&pdu.inner());
                    }
                }
            }

            // if it passes auth now, we can add it to the state
            if crate::validate::auth::auth_check_v1(&*self.db, &event, &frankenstate).await? == AuthStatus::Pass {
                state.insert_event(&event);
            }
        }

        Ok(state)
    }
}

fn is_power_event(pdu: &VersionedPdu) -> bool {
    match pdu.event_content() {
        EventContent::PowerLevels(_) | EventContent::JoinRules(_) => true,
        EventContent::Member(Member { membership: Membership::Leave, .. }) |
            EventContent::Member(Member { membership: Membership::Ban, .. })
            if pdu.state_key().as_deref() != Some(pdu.sender().as_str()) => true,
        _ => false,
    }
}

fn auth_types_for_event(pdu: &VersionedPdu) -> HashSet<(&str, &str)> {
    let mut ret = HashSet::new();
    if pdu.event_content().get_type() == "m.room.create" {
        return ret;
    }

    ret.insert(("m.room.create", ""));
    ret.insert(("m.room.member", &pdu.sender().as_str()));
    ret.insert(("m.room.power_levels", ""));

    if let EventContent::Member(ref member) = pdu.event_content() {
        ret.insert(("m.room.member", pdu.state_key().as_ref().unwrap()));

        let membership = &member.membership;
        if *membership == Membership::Join || *membership == Membership::Invite {
            ret.insert(("m.room.join_rules", ""));
        }

        // TODO: third party invites. see synapse event_auth.py
    }

    ret
}

fn mainline_cmp(x: &(StoredPdu, usize), y: &(StoredPdu, usize)) -> Ordering {
    // list is sorted backwards
    let mainline_based_order = x.1.cmp(&y.1).reverse();
    if mainline_based_order.is_ne() {
        return mainline_based_order;
    }

    // time, however, is not
    let ts_based_order = x.0.origin_server_ts().cmp(&y.0.origin_server_ts());
    if ts_based_order.is_ne() {
        return ts_based_order;
    }

    let id_based_order = x.0.event_id().cmp(&y.0.event_id());
    if id_based_order.is_ne() {
        return id_based_order;
    }

    panic!("oh come on now");
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{storage::{Storage, StorageManager}, error::Error, util::{StorageExt, storage::NewEvent, MatrixId}, events::{room::{Create, Name, Member, Membership}, EventContent, room_version::{v4::UnhashedPdu, VersionedPdu}, pdu::StoredPdu}};

    use super::StateResolver;

    struct TestRoom<'db> {
        db: &'db dyn Storage,
        room_id: String,
        /// depth -> list of events at that depth
        depth_map: Vec<Vec<String>>,
    }

    impl<'a> TestRoom<'a> {
        /// must only be called once per test, because it uses the same room id every time
        async fn create<'db>(
            db: &'db dyn Storage,
            room_id: &str,
            creator: &MatrixId,
        ) -> Result<TestRoom<'db>, Error> {
            let creation = UnhashedPdu {
                event_content: EventContent::Create(Create {
                    creator: creator.clone(),
                    room_version: Some(String::from("4")),
                    predecessor: None,
                    extra: HashMap::new(),
                }),
                room_id: String::from(room_id),
                sender: creator.clone(),
                state_key: Some(String::new()),
                unsigned: None,
                redacts: None,
                origin: String::from("example.org"),
                origin_server_ts: 0,
                prev_events: Vec::new(),
                depth: 0,
                auth_events: Vec::new(),
            }.finalize();
            let creation_id = creation.event_id();
            db.add_pdus(&[StoredPdu {
                inner: VersionedPdu::V4(creation),
                auth_status: crate::validate::auth::AuthStatus::Pass,
            }]).await?;
            Ok(TestRoom {
                db,
                room_id: room_id.to_owned(),
                depth_map: vec![vec![creation_id]],
            })
        }

        /// Like `crate::util::storage::StorageExt::add_event`, but it allows you to specify depth,
        /// which you usually don't want, but is very useful for testing.
        async fn add(
            &mut self,
            depth: usize,
            sender: &MatrixId,
            content: impl Into<EventContent>,
            state_key: Option<&str>,
            state_resolver: &StateResolver,
        ) -> Result<String, Error> {
            let prev_depth = depth.checked_sub(1).unwrap();
            let prev_events = &self.depth_map[prev_depth];
            let state = state_resolver.resolve(&self.room_id, &prev_events).await?;

            let new_event = NewEvent {
                event_content: content.into(),
                sender: sender.clone(),
                state_key: state_key.map(String::from),
                redacts: None,
                unsigned: None,
            };

            let auth_events = crate::util::storage::calc_auth_events(&new_event, &state);
            let pdu = VersionedPdu::V4(UnhashedPdu {
                event_content: new_event.event_content,
                room_id: self.room_id.clone(),
                sender: new_event.sender,
                state_key: new_event.state_key,
                unsigned: None,
                redacts: None,
                origin: String::from("example.org"),
                origin_server_ts: 0,
                prev_events: prev_events.clone(),
                depth: depth as i64,
                auth_events,
            }.finalize());
            let event_id = pdu.event_id();

            if self.depth_map.len() == depth {
                self.depth_map.push(Vec::new());
            } else if self.depth_map.len() < depth {
                panic!("can't insert event there");
            }
            self.depth_map[depth].push(event_id.clone());

            let auth_status = crate::validate::auth::auth_check_v1(self.db, &pdu, &state).await?;
            self.db.add_pdus(&[StoredPdu {
                inner: pdu,
                auth_status,
            }]).await?;

            Ok(event_id)
        }
    }

    async fn construct_cursed_room(db: &dyn Storage, resolver: &StateResolver) -> Result<(), Error> {
        let room_id = "!cursed:example.org";
        let alice = MatrixId::new("alice", "example.org").unwrap();
        db.add_pdus(&[StoredPdu {
            inner: VersionedPdu::V4(UnhashedPdu {
                event_content: EventContent::Create(Create {
                    creator: alice.clone(),
                    room_version: Some(String::from("4")),
                    predecessor: None,
                    extra: HashMap::new(),
                }),
                room_id: String::from(room_id),
                sender: alice.clone(),
                state_key: Some(String::new()),
                unsigned: None,
                redacts: None,
                origin: String::from("example.org"),
                origin_server_ts: 0,
                prev_events: Vec::new(),
                depth: 0,
                auth_events: Vec::new(),
            }.finalize()),
            auth_status: crate::validate::auth::AuthStatus::Pass,
        }]).await?;
        db.add_event(room_id, NewEvent {
            event_content: EventContent::Member(Member {
                avatar_url: None,
                displayname: None,
                membership: Membership::Join,
                is_direct: false,
            }),
            sender: alice.clone(),
            state_key: Some(alice.clone_inner()),
            redacts: None,
            unsigned: None
        }, resolver).await?;
        db.add_event(room_id, NewEvent {
            event_content: EventContent::Name(Name {
                name: String::from("one"),
            }),
            sender: alice.clone(),
            state_key: Some(String::new()),
            redacts: None,
            unsigned: None,
        }, resolver).await?;
        Ok(())
    }

    #[test]
    fn linear() {
        crate::init_tracing();
        let mut rt = tokio::runtime::Builder::new().basic_scheduler().build().unwrap();
        rt.block_on(linear_inner()).unwrap();
    }

    async fn linear_inner() -> Result<(), Error> {
        let storage_manager = crate::storage::mem::MemStorageManager::new();
        let db = storage_manager.get_handle().await?;
        let resolver = StateResolver::new(storage_manager.get_handle().await?);

        let alice = MatrixId::new("alice", "example.org").unwrap();
        let room_id = "!linear:example.org";
        let mut room = TestRoom::create(&*db, room_id, &alice).await?;
        let _alice_join = room.add(1, &alice, Member {
            avatar_url: None,
            displayname: None,
            membership: Membership::Join,
            is_direct: false,
        }, Some(alice.as_str()), &resolver).await?;
        let name1 = room.add(2, &alice, Name {
            name: String::from("one"),
        }, Some(""), &resolver).await?;

        let state1 = resolver.resolve(room_id, &[name1.clone()]).await?;
        assert_eq!(state1.get_content::<Name>(&*db, "").await?.unwrap().name, "one");

        let name2 = room.add(3, &alice, Name {
            name: String::from("two"),
        }, Some(""), &resolver).await?;
        let state2 = resolver.resolve(room_id, &[name2]).await?;
        assert_eq!(state2.get_content::<Name>(&*db, "").await?.unwrap().name, "two");
        let state1 = resolver.resolve(room_id, &[name1]).await?;
        assert_eq!(state1.get_content::<Name>(&*db, "").await?.unwrap().name, "one");
        Ok(())
    }
}
