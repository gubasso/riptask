use crate::config::{Config, parse_priority, parse_state};
use crate::domain::issue::{IssueState, Priority};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::storage::frontmatter;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct TemplateDocument {
    pub name: String,
    pub template_name: String,
    pub default_labels: Vec<String>,
    pub default_status: Option<IssueState>,
    pub default_priority: Option<Priority>,
    pub title_hint: Option<String>,
    pub body: String,
}

pub struct TemplateService<'a> {
    paths: &'a AppPaths,
    config: &'a Config,
}

impl<'a> TemplateService<'a> {
    pub fn new(paths: &'a AppPaths, config: &'a Config) -> Self {
        Self { paths, config }
    }

    pub fn load(&self, name: &str) -> Result<TemplateDocument> {
        anyhow::ensure!(
            !name.contains('/') && !name.contains('\\') && !name.contains("..") && !name.is_empty(),
            "invalid template name: {name}"
        );
        let path = self.paths.templates_dir().join(format!("{name}.md"));
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read template {}", path.as_str()))?;
        let (yaml, body, _) = frontmatter::split(&content, path.as_str())?;
        let value: serde_json::Value = serde_yaml_ng::from_str(&yaml)?;
        let default_labels = value
            .get("default_labels")
            .and_then(|labels| labels.as_array())
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let default_status = value
            .get("default_status")
            .and_then(|state| state.as_str())
            .map(parse_state)
            .transpose()?;
        let default_priority = value
            .get("default_priority")
            .and_then(|priority| priority.as_str())
            .map(map_priority)
            .transpose()?;
        Ok(TemplateDocument {
            name: name.to_owned(),
            template_name: value
                .get("template_name")
                .and_then(|value| value.as_str())
                .unwrap_or(name)
                .to_owned(),
            default_labels,
            default_status,
            default_priority,
            title_hint: None,
            body,
        })
    }

    pub fn list_names(&self) -> Result<Vec<String>, RiptskError> {
        let mut names = Vec::new();
        for entry in fs::read_dir(self.paths.templates_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) == Some("md")
                && let Some(stem) = path.file_stem().and_then(|value| value.to_str())
            {
                names.push(stem.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn validate_file(&self, path: &Path) -> Result<(), RiptskError> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read template {}", path.display()))
            .map_err(RiptskError::Other)?;
        let (yaml, _, _) = frontmatter::split(&content, &path.display().to_string())?;
        let value: serde_json::Value = serde_yaml_ng::from_str(&yaml)
            .map_err(|error| RiptskError::Config(error.to_string()))?;

        let Some(template_name) = value.get("template_name").and_then(|value| value.as_str())
        else {
            return Err(RiptskError::Config("template_name is required".into()));
        };
        if template_name.is_empty() {
            return Err(RiptskError::Config("template_name is required".into()));
        }
        if let Some(state) = value.get("default_status").and_then(|value| value.as_str()) {
            parse_state(state).map_err(|error| RiptskError::Config(error.to_string()))?;
        }
        if let Some(priority) = value
            .get("default_priority")
            .and_then(|value| value.as_str())
        {
            map_priority(priority).map_err(|error| RiptskError::Config(error.to_string()))?;
        }
        Ok(())
    }
}

pub fn seed_template(name: &str) -> String {
    format!(
        "---\ntemplate_name: {name}\ndefault_labels: []\ndefault_status: backlog\ndefault_priority: medium\ntitle_hint: \"\"\n---\n\n## Description\n\n[Describe the issue]\n\n## Tasks\n\n- [ ] ...\n"
    )
}

fn map_priority(value: &str) -> Result<Priority> {
    match value {
        "none" => Ok(Priority::Medium),
        "critical" => Ok(Priority::Urgent),
        other => parse_priority(other),
    }
}
