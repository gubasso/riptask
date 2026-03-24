use crate::domain::issue::IssueState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardConfig {
    pub name: String,
    pub statuses: Vec<IssueState>,
}
