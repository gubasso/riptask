use crate::error::RiptskError;

pub trait PromptBackend {
    fn input(&self, prompt: &str, default: Option<&str>) -> Result<String, RiptskError>;
    fn confirm(&self, prompt: &str, default: bool) -> Result<bool, RiptskError>;
    fn select(&self, prompt: &str, items: &[String], default: usize)
    -> Result<String, RiptskError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DialoguerPrompts;

impl PromptBackend for DialoguerPrompts {
    fn input(&self, prompt: &str, default: Option<&str>) -> Result<String, RiptskError> {
        let mut builder = dialoguer::Input::<String>::new().with_prompt(prompt);
        if let Some(default) = default {
            builder = builder.default(default.to_owned());
        }
        builder
            .interact_text()
            .map_err(|error| RiptskError::General(error.to_string()))
    }

    fn confirm(&self, prompt: &str, default: bool) -> Result<bool, RiptskError> {
        dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(default)
            .interact()
            .map_err(|error| RiptskError::General(error.to_string()))
    }

    fn select(
        &self,
        prompt: &str,
        items: &[String],
        default: usize,
    ) -> Result<String, RiptskError> {
        let index = dialoguer::Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(default)
            .interact()
            .map_err(|error| RiptskError::General(error.to_string()))?;
        items
            .get(index)
            .cloned()
            .ok_or_else(|| RiptskError::General("selection index out of bounds".into()))
    }
}
