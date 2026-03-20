use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendStateEntry {
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub updated_at: String,
}

pub type BackendState = HashMap<String, BackendStateEntry>;

pub fn backend_state_key(provider: &str, repo: &str, issue_id: u64) -> String {
    format!("{provider}:{repo}:{issue_id}")
}

#[cfg(test)]
mod tests {
    use super::backend_state_key;

    #[test]
    fn builds_expected_cache_key() {
        assert_eq!(
            backend_state_key("gitlab", "chrono/wormhole-router", 42),
            "gitlab:chrono/wormhole-router:42"
        );
    }
}
