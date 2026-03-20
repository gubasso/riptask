use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionState {
    pub previous_branch: String,
    pub session_branch: String,
    pub issue_id: Option<String>,
    pub started_at: String,
}
