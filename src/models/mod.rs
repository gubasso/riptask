pub mod ai;
pub mod backend;
pub mod board;
pub mod defaults;
pub mod recurring;
pub mod sync;
pub mod ui;

pub use ai::{AiConfig, AiFeatures};
pub use backend::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
pub use board::BoardConfig;
pub use defaults::DefaultsConfig;
pub use recurring::{RecurrenceFrequency, RecurringDef};
pub use sync::SyncConfig;
pub use ui::UiConfig;
