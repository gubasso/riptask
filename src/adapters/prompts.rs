use crate::error::TskError;

pub trait PromptBackend {
    fn input(&self, prompt: &str, default: Option<&str>) -> Result<String, TskError>;
    fn confirm(&self, prompt: &str, default: bool) -> Result<bool, TskError>;
    fn select(&self, prompt: &str, items: &[String], default: usize) -> Result<String, TskError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DialoguerPrompts;

impl PromptBackend for DialoguerPrompts {
    fn input(&self, prompt: &str, default: Option<&str>) -> Result<String, TskError> {
        let mut builder = dialoguer::Input::<String>::new().with_prompt(prompt);
        if let Some(default) = default {
            builder = builder.default(default.to_owned());
        }
        builder
            .interact_text()
            .map_err(|error| TskError::General(error.to_string()))
    }

    fn confirm(&self, prompt: &str, default: bool) -> Result<bool, TskError> {
        dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(default)
            .interact()
            .map_err(|error| TskError::General(error.to_string()))
    }

    fn select(&self, prompt: &str, items: &[String], default: usize) -> Result<String, TskError> {
        let index = dialoguer::Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(default)
            .interact()
            .map_err(|error| TskError::General(error.to_string()))?;
        items
            .get(index)
            .cloned()
            .ok_or_else(|| TskError::General("selection index out of bounds".into()))
    }
}
