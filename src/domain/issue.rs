use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum IssueState {
    Backlog,
    Todo,
    InProgress,
    Review,
    Done,
}

impl IssueState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in-progress",
            Self::Review => "review",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GitlabIssueMeta {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pushed_state: Option<IssueState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GithubIssueMeta {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pushed_state: Option<IssueState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictMeta {
    pub detected_at: String,
    pub remote_file: String,
    pub remote_updated_at: String,
    pub local_updated_at: String,
    pub last_synced_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueFrontmatter {
    pub id: String,
    pub title: String,
    pub state: IssueState,
    pub board: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gitlab: Option<GitlabIssueMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GithubIssueMeta>,
    pub local_updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<String>,
    #[serde(default)]
    pub remote_deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_parent: Option<String>,
    #[serde(rename = "id-slug")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueDocument {
    pub frontmatter: IssueFrontmatter,
    pub body: String,
    pub remote_section: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::storage::frontmatter;

    #[test]
    fn round_trips_fixture_frontmatter() {
        let input = include_str!("../../tests/fixtures/issues/GL-CHR-WOR--42.md");
        let document = frontmatter::parse_issue_str("fixture", input).expect("parse fixture");
        let output = frontmatter::serialize_issue(&document).expect("serialize fixture");
        let reparsed = frontmatter::parse_issue_str("fixture", &output).expect("reparse fixture");
        assert_eq!(reparsed.frontmatter.id, "GL-CHR-WOR--42");
        assert_eq!(document.frontmatter.id, reparsed.frontmatter.id);
    }
}
