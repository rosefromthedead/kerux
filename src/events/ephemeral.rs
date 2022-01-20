use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::util::MatrixId;

/// `m.typing`
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Typing {
    pub user_ids: HashSet<MatrixId>,
}
