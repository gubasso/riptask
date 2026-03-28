use serde::{Deserialize, Deserializer, Serialize};

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
    pub milestone_id: Option<u64>,
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
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pushed_state: Option<IssueState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueFrontmatter {
    pub id: String,
    pub title: String,
    pub status: IssueState,
    pub board: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(
        default,
        alias = "assignee",
        deserialize_with = "deserialize_assignees"
    )]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
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
    pub weight: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidential: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discussion_locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<String>,
    #[serde(default)]
    pub remote_deleted: bool,
    #[serde(rename = "id-slug")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueDocument {
    pub frontmatter: IssueFrontmatter,
    pub body: String,
    pub remote_section: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AssigneesField {
    Many(Vec<String>),
    One(String),
    Null(()),
}

fn deserialize_assignees<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<AssigneesField>::deserialize(deserializer)? {
        Some(AssigneesField::Many(values)) => Ok(values),
        Some(AssigneesField::One(value)) => Ok(vec![value]),
        Some(AssigneesField::Null(_)) | None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::frontmatter;

    #[test]
    fn deserializes_legacy_assignee_string() {
        let yaml = r#"---
id: TEST--1
title: Legacy assignee
status: todo
board: personal
project: test
assignee: alice
local_updated_at: "2026-03-20T10:00:00Z"
---
body"#;
        let doc = frontmatter::parse_issue_str("test", yaml).expect("parse legacy assignee");
        assert_eq!(doc.frontmatter.assignees, vec!["alice"]);
    }

    #[test]
    fn deserializes_assignees_array() {
        let yaml = r#"---
id: TEST--2
title: Multi assignees
status: todo
board: personal
project: test
assignees:
  - alice
  - bob
local_updated_at: "2026-03-20T10:00:00Z"
---
body"#;
        let doc = frontmatter::parse_issue_str("test", yaml).expect("parse assignees array");
        assert_eq!(doc.frontmatter.assignees, vec!["alice", "bob"]);
    }

    #[test]
    fn deserializes_missing_assignee_as_empty() {
        let yaml = r#"---
id: TEST--3
title: No assignee
status: todo
board: personal
project: test
local_updated_at: "2026-03-20T10:00:00Z"
---
body"#;
        let doc = frontmatter::parse_issue_str("test", yaml).expect("parse no assignee");
        assert!(doc.frontmatter.assignees.is_empty());
    }

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
