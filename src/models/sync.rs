use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SyncConfig {
    #[serde(default = "default_true")]
    pub conflict_detection: bool,
}

pub(crate) fn default_true() -> bool {
    true
}
