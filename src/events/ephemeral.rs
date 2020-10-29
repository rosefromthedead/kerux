use serde::{Deserialize, Serialize, ser::SerializeStruct};
use std::{time::Instant, collections::{HashMap, HashSet}};

use crate::util::MatrixId;

/// `m.typing`
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Typing {
    pub user_ids: HashSet<MatrixId>,
}
