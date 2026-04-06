use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkCloneMarker {
    pub main_repo_path: String,
    pub branch: String,
    pub issue_id: String,
    pub remote_url: String,
}
