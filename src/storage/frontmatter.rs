use crate::domain::issue::{IssueDocument, IssueFrontmatter};
use crate::error::FrontmatterError;
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use std::fs;
use std::io::Write;
use std::path::Path;

const REMOTE_START: &str = "<!-- TSK:REMOTE:START";
pub const CONFLICT_MARKER: &str = "<<<<<<< LOCAL";

pub fn split(
    content: &str,
    path: &str,
) -> Result<(String, String, Option<String>), FrontmatterError> {
    if !content.starts_with("---\n") {
        return Err(FrontmatterError::MissingDelimiters {
            path: path.to_owned(),
        });
    }

    let mut offset = 4usize;
    let rest = &content[offset..];
    let end = rest
        .find("\n---\n")
        .ok_or_else(|| FrontmatterError::MissingDelimiters {
            path: path.to_owned(),
        })?;
    let yaml = rest[..end].to_owned();
    offset += end + 5;
    let body_plus_remote = &content[offset..];

    if let Some(start) = body_plus_remote.find(REMOTE_START) {
        let body = body_plus_remote[..start].to_owned();
        let remote = body_plus_remote[start..].to_owned();
        Ok((yaml, body, Some(remote)))
    } else {
        Ok((yaml, body_plus_remote.to_owned(), None))
    }
}

pub fn join(yaml: &str, body: &str, remote_section: Option<&str>) -> String {
    let mut output = String::from("---\n");
    output.push_str(yaml.trim_start_matches("---\n"));
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("---\n");
    output.push_str(body);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    if let Some(remote) = remote_section {
        output.push_str(remote);
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    output
}

pub fn parse_issue_str(path: &str, content: &str) -> Result<IssueDocument> {
    let (yaml, body, remote_section) = split(content, path)?;
    let frontmatter: IssueFrontmatter =
        serde_yaml_ng::from_str(&yaml).map_err(|error| FrontmatterError::InvalidYaml {
            path: path.to_owned(),
            detail: error.to_string(),
        })?;
    Ok(IssueDocument {
        frontmatter,
        body,
        remote_section,
    })
}

pub fn load_issue(path: &Path) -> Result<IssueDocument> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_issue_str(&path.display().to_string(), &content)
}

pub fn has_conflict_markers(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.starts_with(CONFLICT_MARKER))
}

pub enum IssueLoadResult {
    Ok(Box<IssueDocument>),
    Conflict { id: String, path: Utf8PathBuf },
    Err(anyhow::Error),
}

pub fn try_load_issue(path: &Path) -> IssueLoadResult {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => return IssueLoadResult::Err(error.into()),
    };
    if has_conflict_markers(&content) {
        let id = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let utf8_path = Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap_or_default();
        return IssueLoadResult::Conflict {
            id,
            path: utf8_path,
        };
    }
    match parse_issue_str(&path.display().to_string(), &content) {
        Ok(document) => IssueLoadResult::Ok(Box::new(document)),
        Err(error) => IssueLoadResult::Err(error),
    }
}

pub fn serialize_issue(document: &IssueDocument) -> Result<String> {
    let yaml = serde_yaml_ng::to_string(&document.frontmatter)
        .context("failed to serialize issue frontmatter")?;
    Ok(join(
        &yaml,
        &document.body,
        document.remote_section.as_deref(),
    ))
}

pub fn save_issue(path: &Path, document: &IssueDocument) -> Result<()> {
    let content = serialize_issue(document)?;
    let dir = path.parent().context("issue path has no parent")?;
    let mut file =
        tempfile::NamedTempFile::new_in(dir).context("failed to create temp issue file")?;
    file.write_all(content.as_bytes())
        .context("failed to write temp issue file")?;
    file.persist(path)
        .map_err(|error| anyhow::Error::from(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CONFLICT_MARKER, has_conflict_markers, join, parse_issue_str, split};

    #[test]
    fn preserves_yaml_like_body_content() {
        let content = "---\nid: TEST-001\nstatus: todo\nboard: personal\nproject: demo\nlocal_updated_at: 2026-03-18T00:00:00Z\nlabels: []\nremote_deleted: false\n---\n## Example\n\n```yaml\n---\nfake: frontmatter\n---\n```\n";
        let (_, body, _) = split(content, "inline").expect("split content");
        assert!(body.contains("fake: frontmatter"));
    }

    #[test]
    fn extracts_remote_section() {
        let content = include_str!("../../tests/fixtures/issues/GL-CHR-WOR--42.md");
        let document = parse_issue_str("fixture", content).expect("parse issue");
        assert!(document.remote_section.is_none());
    }

    #[test]
    fn join_always_ends_with_newline() {
        let output = join("id: TEST-001\n", "body", None);
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn detects_conflict_markers() {
        assert!(has_conflict_markers(CONFLICT_MARKER));
        assert!(has_conflict_markers(&format!(
            "line one\n{CONFLICT_MARKER}\nline three"
        )));
        assert!(!has_conflict_markers("clean content"));
        assert!(!has_conflict_markers(&format!("# {CONFLICT_MARKER}")));
        assert!(!has_conflict_markers(&format!("> {CONFLICT_MARKER}")));
        assert!(!has_conflict_markers(&format!("  {CONFLICT_MARKER}")));
    }
}
