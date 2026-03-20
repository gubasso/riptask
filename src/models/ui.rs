use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fzf_opts: Option<String>,
}
