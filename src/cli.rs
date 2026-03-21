use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "tsk",
    version,
    about = "Plaintext issue tracker — manage issues, boards, and sync from the terminal",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ScopeArgs {
    /// Limit to specific projects
    #[arg(long = "project", short = 'p')]
    pub projects: Vec<String>,
    /// Run across all registered projects
    #[arg(long = "all-projects", short = 'a')]
    pub all_projects: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct IdArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Issue ID (or pick interactively if omitted)
    pub id: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a new tsk repository
    Init,
    /// Create a new issue
    New(NewArgs),
    /// Edit an existing issue in your editor
    Edit(IdArgs),
    /// Display issue details
    Show(IdArgs),
    /// Move an issue to a different state
    #[command(name = "move")]
    Move(MoveArgs),
    /// Close an issue
    Close(IdArgs),
    /// Reopen a closed issue
    Reopen(IdArgs),
    /// Remove an issue permanently
    Rm(IdArgs),
    /// List issues with optional filters
    Ls(LsArgs),
    /// Print the file path of an issue
    Path(IdArgs),
    /// Create, switch to, or delete an issue branch
    Branch(BranchArgs),
    /// Create a pull request for an issue
    Pr(IdArgs),
    /// Display or manage board views
    Board(BoardArgs),
    /// Regenerate all board view files
    View,
    /// Set the order of issues in a state
    Reorder(ReorderArgs),
    /// Move an issue one position up in its state
    #[command(name = "reorder-up")]
    ReorderUp(IdArgs),
    /// Move an issue one position down in its state
    #[command(name = "reorder-down")]
    ReorderDown(IdArgs),
    /// Sync issues with a remote backend (GitHub/GitLab)
    Sync(SyncArgs),
    /// Push local issues to their remote backend
    Push(PushArgs),
    /// Resolve a sync conflict on an issue
    Resolve(ResolveArgs),
    /// Start or end a work session
    Session(SessionArgs),
    /// Commit tsk issue changes to git
    Commit(CommitArgs),
    /// Manage recurring issue schedules
    Recur(RecurArgs),
    /// Manage issue templates
    Template(TemplateArgs),
    /// Register a backend project (GitHub/GitLab)
    Register(RegisterArgs),
    /// Generate an AI summary of issues
    Summarize(SummarizeArgs),
    /// Ask an AI question about your issues
    Ask(AskArgs),
    /// View or update tsk configuration
    Config(ConfigArgs),
    /// Manage git hooks for auto-commit
    Hooks(HooksArgs),
    /// Generate shell completions
    #[command(hide = true)]
    Completions {
        /// Target shell
        shell: CompletionShell,
    },
    /// Print version information
    Version,
    /// Show help for a command
    Help {
        /// Command name
        command: Option<String>,
    },
}

#[derive(Debug, Clone, Args, Default)]
pub struct BranchArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,

    /// Target branch name or issue ID
    pub id: Option<String>,

    /// Delete branch (safe — must be fully merged)
    #[arg(short = 'd', long = "delete")]
    pub delete: bool,

    /// Force-delete branch (even if not merged)
    #[arg(short = 'D', long = "force-delete", conflicts_with = "delete")]
    pub force_delete: bool,

    /// Skip confirmation prompt (for -d/-D)
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct NewArgs {
    /// Issue title (or enter interactively)
    #[arg(long)]
    pub title: Option<String>,
    /// Target project
    #[arg(long)]
    pub project: Option<String>,
    /// Board to assign
    #[arg(long)]
    pub board: Option<String>,
    /// Initial state
    #[arg(long)]
    pub state: Option<String>,
    /// Priority level
    #[arg(long)]
    pub priority: Option<String>,
    /// Template to use
    #[arg(long, short = 't')]
    pub template: Option<String>,
    /// Generate issue content with AI
    #[arg(long)]
    pub ai: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct MoveArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Issue ID to move
    pub id: Option<String>,
    /// Target state
    pub state: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct LsArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Filter by state
    #[arg(long)]
    pub state: Option<String>,
    /// Filter by priority
    #[arg(long)]
    pub priority: Option<String>,
    /// Filter by board
    #[arg(long)]
    pub board: Option<String>,
    /// Filter by cycle
    #[arg(long)]
    pub cycle: Option<String>,
    /// Filter by assignee
    #[arg(long)]
    pub assignee: Option<String>,
    /// Include closed/done issues
    #[arg(long = "all")]
    pub include_done: bool,
    /// Show only issues with sync conflicts
    #[arg(long)]
    pub conflicts: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct BoardArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Show all boards including inactive
    #[arg(long)]
    pub all: bool,
    /// Target a specific board
    #[arg(long)]
    pub board: Option<String>,
    /// Open the board in the default viewer
    #[arg(long)]
    pub open: bool,
    /// Print the board file path
    #[arg(long)]
    pub path: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ReorderArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Board to reorder within
    #[arg(long)]
    pub board: Option<String>,
    /// State column to reorder
    pub state: String,
    /// Issue IDs in desired order
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub subcommand: Option<SyncSubcommand>,
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Sync all projects
    #[arg(long)]
    pub all: bool,
    /// Target a specific backend by name
    #[arg(long = "backend")]
    pub backend: Option<String>,
    /// Overwrite remote state on conflicts
    #[arg(long)]
    pub force: bool,
    /// Show triage prompt for new remote issues
    #[arg(long)]
    pub triage: bool,
    /// Automatically triage new remote issues
    #[arg(long = "auto-triage")]
    pub auto_triage: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SyncSubcommand {
    /// Pull issues from the remote backend
    Pull,
    /// Push local changes to the remote backend
    Push,
    /// Show sync status
    Status,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PushArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Push all issues
    #[arg(long)]
    pub all: bool,
    /// Issue IDs to push
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ResolveArgs {
    /// Issue ID with the conflict
    pub id: String,
    /// Keep the remote version
    #[arg(long = "take-remote")]
    pub take_remote: bool,
    /// Keep the local version
    #[arg(long = "take-local")]
    pub take_local: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub subcommand: SessionSubcommand,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SessionStartArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Issue ID to focus the session on
    pub id: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SessionSubcommand {
    /// Start a work session
    Start(SessionStartArgs),
    /// End the active work session
    End,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Clone, Args, Default)]
pub struct CommitArgs {
    /// Open the commit message in an editor
    #[arg(short = 'e', long)]
    pub edit: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct RecurArgs {
    #[command(subcommand)]
    pub subcommand: Option<RecurSubcommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum RecurSubcommand {
    /// List recurring definitions
    List,
    /// Create a new recurring definition
    New(Box<RecurNewArgs>),
    /// Run pending recurrences
    Run {
        /// Date to generate for (defaults to today)
        date: Option<String>,
    },
    /// Skip the next occurrence
    Skip {
        /// Recurring definition ID to skip
        recur_id: Option<String>,
    },
}

#[derive(Debug, Clone, Args, Default)]
pub struct RecurNewArgs {
    /// Unique ID for this recurring definition
    #[arg(long)]
    pub id: String,
    /// Template to use for generated issues
    #[arg(long)]
    pub template: Option<String>,
    /// Title pattern (supports date placeholders)
    #[arg(long)]
    pub title_pattern: String,
    /// Recurrence frequency (daily, weekly, monthly)
    #[arg(long)]
    pub frequency: String,
    /// Board for generated issues
    #[arg(long)]
    pub board: Option<String>,
    /// Project for generated issues
    #[arg(long)]
    pub project: Option<String>,
    /// Organization for generated issues
    #[arg(long)]
    pub org: Option<String>,
    /// Initial state for generated issues
    #[arg(long)]
    pub state: Option<String>,
    /// Priority for generated issues
    #[arg(long)]
    pub priority: Option<String>,
    /// Assignee for generated issues
    #[arg(long)]
    pub assignee: Option<String>,
    /// Labels for generated issues
    #[arg(long)]
    pub labels: Vec<String>,
    /// Day of week for weekly recurrences
    #[arg(long)]
    pub day_of_week: Option<String>,
    /// Day of month for monthly recurrences
    #[arg(long)]
    pub day_of_month: Option<u32>,
    /// Start date for the recurrence schedule
    #[arg(long)]
    pub start: Option<String>,
    /// End date for the recurrence schedule
    #[arg(long)]
    pub end: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct TemplateArgs {
    #[command(subcommand)]
    pub subcommand: Option<TemplateSubcommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TemplateSubcommand {
    /// List available templates
    List,
    /// Show a template's contents
    Show {
        /// Template name
        name: Option<String>,
    },
    /// Create a new template
    New {
        /// Template name
        name: Option<String>,
    },
    /// Edit an existing template
    Edit {
        /// Template name
        name: Option<String>,
    },
    /// Remove a template
    Rm {
        /// Template name
        name: Option<String>,
    },
    /// Validate a template
    Validate {
        /// Template name
        name: Option<String>,
    },
}

#[derive(Debug, Clone, Args, Default)]
pub struct RegisterArgs {
    /// List registered backend projects
    #[arg(long)]
    pub list: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SummarizeArgs {
    /// Limit summary to a specific cycle
    #[arg(long)]
    pub cycle: Option<String>,
    /// Limit summary to a specific board
    #[arg(long)]
    pub board: Option<String>,
    /// Limit summary to a specific project
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct AskArgs {
    /// Natural-language question about your issues
    pub question: String,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub subcommand: Option<ConfigSubcommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigSubcommand {
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Value to set
        value: String,
    },
}

#[derive(Debug, Clone, Args, Default)]
pub struct HooksArgs {
    #[command(subcommand)]
    pub subcommand: Option<HooksSubcommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum HooksSubcommand {
    /// Install git hooks for auto-commit
    Install {
        /// Overwrite existing hooks
        #[arg(long)]
        force: bool,
    },
    /// Update installed hooks to the latest version
    Update {
        /// Overwrite even if already up to date
        #[arg(long)]
        force: bool,
    },
    /// Show hook installation status
    Status,
    /// Remove installed git hooks
    Uninstall {
        /// Remove without confirmation
        #[arg(long)]
        force: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands, SessionSubcommand};
    use clap::Parser;

    #[test]
    fn ls_short_all_projects_parses() {
        let cli = Cli::try_parse_from(["tsk", "ls", "-a"]).expect("parse");
        let Commands::Ls(args) = cli.command.expect("command") else {
            panic!("expected ls command");
        };
        assert!(args.scope.all_projects);
    }

    #[test]
    fn ls_long_all_projects_parses() {
        let cli = Cli::try_parse_from(["tsk", "ls", "--all-projects"]).expect("parse");
        let Commands::Ls(args) = cli.command.expect("command") else {
            panic!("expected ls command");
        };
        assert!(args.scope.all_projects);
    }

    #[test]
    fn ls_project_parses_into_scope() {
        let cli = Cli::try_parse_from(["tsk", "ls", "-p", "foo"]).expect("parse");
        let Commands::Ls(args) = cli.command.expect("command") else {
            panic!("expected ls command");
        };
        assert_eq!(args.scope.projects, vec!["foo"]);
    }

    #[test]
    fn uppercase_all_projects_no_longer_parses() {
        let result = Cli::try_parse_from(["tsk", "ls", "-A"]);
        assert!(result.is_err());
    }

    #[test]
    fn session_start_id_parses() {
        let cli = Cli::try_parse_from(["tsk", "session", "start", "61"]).expect("parse");
        let Commands::Session(args) = cli.command.expect("command") else {
            panic!("expected session command");
        };
        let SessionSubcommand::Start(args) = args.subcommand else {
            panic!("expected session start subcommand");
        };
        assert_eq!(args.id.as_deref(), Some("61"));
    }

    #[test]
    fn session_start_scope_parses() {
        let cli = Cli::try_parse_from(["tsk", "session", "start", "-p", "foo"]).expect("parse");
        let Commands::Session(args) = cli.command.expect("command") else {
            panic!("expected session command");
        };
        let SessionSubcommand::Start(args) = args.subcommand else {
            panic!("expected session start subcommand");
        };
        assert_eq!(args.scope.projects, vec!["foo"]);
        assert_eq!(args.id, None);
    }

    #[test]
    fn session_start_empty_parses() {
        let cli = Cli::try_parse_from(["tsk", "session", "start"]).expect("parse");
        let Commands::Session(args) = cli.command.expect("command") else {
            panic!("expected session command");
        };
        let SessionSubcommand::Start(args) = args.subcommand else {
            panic!("expected session start subcommand");
        };
        assert_eq!(args.id, None);
        assert!(args.scope.projects.is_empty());
        assert!(!args.scope.all_projects);
    }
}
