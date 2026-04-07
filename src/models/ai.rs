use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub features: AiFeatures,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AiFeatures {
    #[serde(default)]
    pub new_body_gen: bool,
    #[serde(default)]
    pub triage: bool,
    #[serde(default)]
    pub summarize: bool,
    #[serde(default)]
    pub ask: bool,
    #[serde(default = "default_true")]
    pub commit: bool,
}

fn default_true() -> bool {
    true
}
