use crate::adapters::backend::MergeMethod;
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
    #[arg(short = 'i', long = "pick")]
    pub pick: bool,
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
    /// Change the status of an issue
    #[command(name = "status")]
    Status(StatusArgs),
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
    /// Create a work-clone for an issue
    Clone(CloneArgs),
    /// Push changes and remove the current work-clone
    Unclone(UncloneArgs),
    /// Complete an issue: merge PR, clean up branches, close issue (convenience wrapper — see `tsk pr merge`, `tsk branch -D`, `tsk close`)
    Done(DoneArgs),
    /// Start working on a new issue: create issue, branch, and PR (convenience wrapper — see `tsk new`, `tsk branch`, `tsk pr`)
    Start(StartArgs),
    /// Create, edit, or show a pull/merge request
    Pr(PrArgs),
    /// Display or manage board views
    Board(BoardArgs),
    /// Regenerate all board view files
    View,
    /// Set the order of issues by status
    Reorder(ReorderArgs),
    /// Move an issue one position up in its status
    #[command(name = "reorder-up")]
    ReorderUp(IdArgs),
    /// Move an issue one position down in its status
    #[command(name = "reorder-down")]
    ReorderDown(IdArgs),
    /// Sync issues with a remote backend (GitHub/GitLab)
    Sync(SyncArgs),
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

    #[arg(short = 'i', long = "pick", conflicts_with_all = ["delete", "force_delete"])]
    pub pick: bool,

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
pub struct CloneArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    #[arg(short = 'i', long = "pick")]
    pub pick: bool,
    /// Issue ID
    pub id: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct UncloneArgs {
    /// Discard uncommitted changes before pushing
    #[arg(short = 'f', long)]
    pub force: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PrArgs {
    #[command(subcommand)]
    pub subcommand: Option<PrSubcommand>,
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Issue ID (shorthand for `tsk pr create [ID]`)
    pub id: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct DoneArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    #[arg(short = 'i', long = "pick")]
    pub pick: bool,
    /// Issue ID (or pick interactively)
    pub id: Option<String>,
    /// Merge method (merge, squash, rebase)
    #[arg(long, value_enum)]
    pub merge_method: Option<MergeMethod>,
    /// Skip waiting for CI checks before merging
    #[arg(long)]
    pub auto_merge: bool,
    /// Skip confirmation prompts
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
    /// Timeout in seconds for waiting (default 600)
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,
    /// Use --force instead of --force-with-lease when pushing rebased branches
    #[arg(long)]
    pub force_push: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct StartArgs {
    /// Issue title (positional)
    #[arg(conflicts_with = "title")]
    pub title_pos: Option<String>,
    /// Issue title (or enter interactively)
    #[arg(short = 't', long)]
    pub title: Option<String>,
    /// Target project
    #[arg(short = 'p', long)]
    pub project: Option<String>,
    /// Board to assign
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Initial status
    #[arg(short = 's', long)]
    pub status: Option<String>,
    /// Priority level
    #[arg(short = 'P', long)]
    pub priority: Option<String>,
    /// Template to use
    #[arg(short = 'T', long)]
    pub template: Option<String>,
    /// Disable AI-assisted content generation (AI is on by default)
    #[arg(long)]
    pub no_ai: bool,
    /// Open in $EDITOR after creation
    #[arg(short = 'e', long)]
    pub edit: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum PrSubcommand {
    /// Create a pull/merge request for an issue
    Create(PrCreateArgs),
    /// Edit an existing pull/merge request
    Edit(PrEditArgs),
    /// Show pull/merge request details
    Show(PrShowArgs),
    /// Merge a pull/merge request
    Merge(PrMergeArgs),
}

#[derive(Debug, Clone, Args, Default)]
pub struct PrCreateArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    pub id: Option<String>,
    /// Skip AI-assisted description generation
    #[arg(long)]
    pub no_ai: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PrEditArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    pub id: Option<String>,
    /// Set PR title directly
    #[arg(short = 't', long)]
    pub title: Option<String>,
    /// Set PR description directly
    #[arg(short = 'd', long)]
    pub description: Option<String>,
    /// Accept AI-generated update without editor review
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Edit manually without AI assistance
    #[arg(long)]
    pub no_ai: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PrShowArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    pub id: Option<String>,
    /// Output in JSON format
    #[arg(short = 'j', long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PrMergeArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    pub id: Option<String>,
    /// Merge method (merge, squash, rebase)
    #[arg(long, value_enum)]
    pub merge_method: Option<MergeMethod>,
    /// Skip waiting for CI checks before merging
    #[arg(long)]
    pub auto_merge: bool,
    /// Skip confirmation prompts
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
    /// Timeout in seconds for waiting (default 600)
    #[arg(long, default_value_t = 600)]
    pub timeout: u64,
    /// Use --force instead of --force-with-lease when pushing rebased branches
    #[arg(long)]
    pub force_push: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct NewArgs {
    /// Issue title (positional)
    #[arg(conflicts_with = "title")]
    pub title_pos: Option<String>,
    /// Issue title (or enter interactively)
    #[arg(short = 't', long)]
    pub title: Option<String>,
    /// Target project
    #[arg(short = 'p', long)]
    pub project: Option<String>,
    /// Board to assign
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Initial status
    #[arg(short = 's', long)]
    pub status: Option<String>,
    /// Priority level
    #[arg(short = 'P', long)]
    pub priority: Option<String>,
    /// Template to use
    #[arg(short = 'T', long)]
    pub template: Option<String>,
    /// Generate issue content with AI
    #[arg(long)]
    pub ai: bool,
    /// Open in $EDITOR after creation
    #[arg(short = 'e', long)]
    pub edit: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct StatusArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    #[arg(short = 'i', long = "pick")]
    pub pick: bool,
    /// Issue ID
    pub id: Option<String>,
    /// Target status
    pub status: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct LsArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Filter by status
    #[arg(short = 's', long)]
    pub status: Option<String>,
    /// Filter by priority
    #[arg(short = 'P', long)]
    pub priority: Option<String>,
    /// Filter by board
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Filter by cycle
    #[arg(short = 'c', long)]
    pub cycle: Option<String>,
    /// Filter by assignee
    #[arg(long)]
    pub assignee: Option<String>,
    /// Include closed/done issues
    #[arg(short = 'A', long = "all")]
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
    #[arg(short = 'A', long)]
    pub all: bool,
    /// Target a specific board
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Open the board in the default viewer
    #[arg(short = 'o', long)]
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
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Status column to reorder
    pub status: String,
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
    #[arg(short = 'A', long)]
    pub all: bool,
    /// Target a specific backend by name
    #[arg(short = 'b', long = "backend")]
    pub backend: Option<String>,
    /// Overwrite remote state on conflicts
    #[arg(short = 'f', long)]
    pub force: bool,
    /// Show triage prompt for new remote issues
    #[arg(short = 't', long)]
    pub triage: bool,
    /// Automatically triage new remote issues
    #[arg(short = 'T', long = "auto-triage")]
    pub auto_triage: bool,
    /// Issue IDs to force-pull (skip conflict detection). Not exposed as CLI flag.
    #[arg(skip)]
    pub force_pull_ids: Vec<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SyncSubcommand {
    /// Pull issues from the remote backend
    Pull(SyncPullPushArgs),
    /// Push local changes to the remote backend
    Push(SyncPullPushArgs),
    /// Show sync status
    Status,
    /// Resolve a sync conflict
    Resolve(ResolveArgs),
}

#[derive(Debug, Clone, Args, Default)]
pub struct SyncPullPushArgs {
    /// Issue IDs to sync (omit for all)
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ResolveArgs {
    /// Issue ID with the conflict
    pub id: String,
    /// Keep the remote version
    #[arg(short = 'r', long = "take-remote")]
    pub take_remote: bool,
    /// Keep the local version
    #[arg(short = 'l', long = "take-local")]
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
    /// Initial status for generated issues
    #[arg(long)]
    pub status: Option<String>,
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
    #[arg(short = 'c', long)]
    pub cycle: Option<String>,
    /// Limit summary to a specific board
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Limit summary to a specific project
    #[arg(short = 'p', long)]
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
    use super::{Cli, Commands, PrSubcommand, SessionSubcommand, SyncSubcommand};
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
    fn ls_short_all_parses() {
        let cli = Cli::try_parse_from(["tsk", "ls", "-A"]).expect("parse");
        let Commands::Ls(args) = cli.command.expect("command") else {
            panic!("expected ls command");
        };
        assert!(args.include_done);
        assert!(!args.scope.all_projects);
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

    #[test]
    fn pr_empty_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        assert!(args.subcommand.is_none());
        assert!(args.id.is_none());
    }

    #[test]
    fn pr_shorthand_id_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "42"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        assert!(args.subcommand.is_none());
        assert_eq!(args.id.as_deref(), Some("42"));
    }

    #[test]
    fn pr_create_no_ai_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "create", "42", "--no-ai"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Create(create)) = args.subcommand else {
            panic!("expected pr create subcommand");
        };
        assert_eq!(create.id.as_deref(), Some("42"));
        assert!(create.no_ai);
    }

    #[test]
    fn pr_create_no_ai_without_id_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "create", "--no-ai"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Create(create)) = args.subcommand else {
            panic!("expected pr create subcommand");
        };
        assert!(create.id.is_none());
        assert!(create.no_ai);
    }

    #[test]
    fn new_short_title_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "-t", "hello"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert_eq!(args.title.as_deref(), Some("hello"));
        assert!(args.title_pos.is_none());
    }

    #[test]
    fn new_positional_title_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "hello"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert_eq!(args.title_pos.as_deref(), Some("hello"));
        assert!(args.title.is_none());
    }

    #[test]
    fn new_positional_and_flag_title_conflict() {
        let err = Cli::try_parse_from(["tsk", "new", "pos", "-t", "flag"]).expect_err("conflict");
        let rendered = err.to_string();
        assert!(rendered.contains("--title"));
        assert!(rendered.contains("cannot be used"));
    }

    #[test]
    fn new_short_template_uppercase_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "-T", "bug"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert_eq!(args.template.as_deref(), Some("bug"));
    }

    #[test]
    fn new_edit_flag_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "-t", "hello", "-e"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert!(args.edit);
    }

    #[test]
    fn start_positional_title_parses() {
        let cli = Cli::try_parse_from(["tsk", "start", "my title"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert_eq!(args.title_pos.as_deref(), Some("my title"));
        assert!(args.title.is_none());
    }

    #[test]
    fn start_no_ai_parses() {
        let cli = Cli::try_parse_from(["tsk", "start", "--no-ai", "my title"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert_eq!(args.title_pos.as_deref(), Some("my title"));
        assert!(args.no_ai);
    }

    #[test]
    fn start_edit_flag_parses() {
        let cli = Cli::try_parse_from(["tsk", "start", "-e", "my title"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert_eq!(args.title_pos.as_deref(), Some("my title"));
        assert!(args.edit);
    }

    #[test]
    fn start_project_parses() {
        let cli = Cli::try_parse_from(["tsk", "start", "-p", "myproject"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert_eq!(args.project.as_deref(), Some("myproject"));
    }

    #[test]
    fn ls_short_status_parses() {
        let cli = Cli::try_parse_from(["tsk", "ls", "-s", "todo"]).expect("parse");
        let Commands::Ls(args) = cli.command.expect("command") else {
            panic!("expected ls command");
        };
        assert_eq!(args.status.as_deref(), Some("todo"));
    }

    #[test]
    fn status_command_parses() {
        let cli = Cli::try_parse_from(["tsk", "status", "42", "in-progress"]).expect("parse");
        let Commands::Status(args) = cli.command.expect("command") else {
            panic!("expected status command");
        };
        assert_eq!(args.id.as_deref(), Some("42"));
        assert_eq!(args.status.as_deref(), Some("in-progress"));
    }

    #[test]
    fn show_long_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "show", "--pick"]).expect("parse");
        let Commands::Show(args) = cli.command.expect("command") else {
            panic!("expected show command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
    }

    #[test]
    fn show_short_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "show", "-i"]).expect("parse");
        let Commands::Show(args) = cli.command.expect("command") else {
            panic!("expected show command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
    }

    #[test]
    fn edit_short_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "edit", "-i"]).expect("parse");
        let Commands::Edit(args) = cli.command.expect("command") else {
            panic!("expected edit command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
    }

    #[test]
    fn done_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "done", "--pick"]).expect("parse");
        let Commands::Done(args) = cli.command.expect("command") else {
            panic!("expected done command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
    }

    #[test]
    fn clone_short_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "clone", "-i"]).expect("parse");
        let Commands::Clone(args) = cli.command.expect("command") else {
            panic!("expected clone command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
    }

    #[test]
    fn status_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "status", "--pick"]).expect("parse");
        let Commands::Status(args) = cli.command.expect("command") else {
            panic!("expected status command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
        assert!(args.status.is_none());
    }

    #[test]
    fn branch_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "branch", "--pick"]).expect("parse");
        let Commands::Branch(args) = cli.command.expect("command") else {
            panic!("expected branch command");
        };
        assert!(args.pick);
        assert!(args.id.is_none());
    }

    #[test]
    fn branch_delete_and_pick_conflict() {
        let err = Cli::try_parse_from(["tsk", "branch", "-d", "--pick"]).expect_err("conflict");
        let rendered = err.to_string();
        assert!(rendered.contains("--pick"));
        assert!(rendered.contains("--delete"));
    }

    #[test]
    fn ls_short_board_parses() {
        let cli = Cli::try_parse_from(["tsk", "ls", "-b", "main"]).expect("parse");
        let Commands::Ls(args) = cli.command.expect("command") else {
            panic!("expected ls command");
        };
        assert_eq!(args.board.as_deref(), Some("main"));
    }

    #[test]
    fn pr_edit_title_and_description_parse() {
        let cli = Cli::try_parse_from([
            "tsk",
            "pr",
            "edit",
            "42",
            "--title",
            "x",
            "--description",
            "y",
        ])
        .expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Edit(edit)) = args.subcommand else {
            panic!("expected pr edit subcommand");
        };
        assert_eq!(edit.id.as_deref(), Some("42"));
        assert_eq!(edit.title.as_deref(), Some("x"));
        assert_eq!(edit.description.as_deref(), Some("y"));
    }

    #[test]
    fn pr_edit_short_title_parses() {
        let cli =
            Cli::try_parse_from(["tsk", "pr", "edit", "42", "-t", "new title"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Edit(edit)) = args.subcommand else {
            panic!("expected pr edit subcommand");
        };
        assert_eq!(edit.title.as_deref(), Some("new title"));
    }

    #[test]
    fn pr_edit_no_ai_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "edit", "42", "--no-ai"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Edit(edit)) = args.subcommand else {
            panic!("expected pr edit subcommand");
        };
        assert_eq!(edit.id.as_deref(), Some("42"));
        assert!(edit.no_ai);
    }

    #[test]
    fn pr_edit_yes_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "edit", "42", "-y"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Edit(edit)) = args.subcommand else {
            panic!("expected pr edit subcommand");
        };
        assert_eq!(edit.id.as_deref(), Some("42"));
        assert!(edit.yes);
    }

    #[test]
    fn pr_edit_yes_and_no_ai_both_parse() {
        let cli = Cli::try_parse_from(["tsk", "pr", "edit", "42", "-y", "--no-ai"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Edit(edit)) = args.subcommand else {
            panic!("expected pr edit subcommand");
        };
        assert!(edit.yes);
        assert!(edit.no_ai);
    }

    #[test]
    fn pr_show_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "show", "42"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Show(show)) = args.subcommand else {
            panic!("expected pr show subcommand");
        };
        assert_eq!(show.id.as_deref(), Some("42"));
        assert!(!show.json);
    }

    #[test]
    fn pr_show_json_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "show", "42", "--json"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Show(show)) = args.subcommand else {
            panic!("expected pr show subcommand");
        };
        assert_eq!(show.id.as_deref(), Some("42"));
        assert!(show.json);
    }

    #[test]
    fn pr_show_short_json_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "show", "42", "-j"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        let Some(PrSubcommand::Show(show)) = args.subcommand else {
            panic!("expected pr show subcommand");
        };
        assert!(show.json);
    }

    #[test]
    fn sync_short_force_parses() {
        let cli = Cli::try_parse_from(["tsk", "sync", "-f"]).expect("parse");
        let Commands::Sync(args) = cli.command.expect("command") else {
            panic!("expected sync command");
        };
        assert!(args.force);
    }

    #[test]
    fn sync_pull_ids_parse() {
        let cli = Cli::try_parse_from(["tsk", "sync", "pull", "42", "43"]).expect("parse");
        let Commands::Sync(args) = cli.command.expect("command") else {
            panic!("expected sync command");
        };
        let Some(SyncSubcommand::Pull(args)) = args.subcommand else {
            panic!("expected sync pull subcommand");
        };
        assert_eq!(args.ids, vec!["42", "43"]);
    }

    #[test]
    fn sync_resolve_short_flags_parse() {
        let cli = Cli::try_parse_from(["tsk", "sync", "resolve", "42", "-r"]).expect("parse");
        let Commands::Sync(args) = cli.command.expect("command") else {
            panic!("expected sync command");
        };
        let Some(SyncSubcommand::Resolve(args)) = args.subcommand else {
            panic!("expected sync resolve subcommand");
        };
        assert!(args.take_remote);

        let cli = Cli::try_parse_from(["tsk", "sync", "resolve", "42", "-l"]).expect("parse");
        let Commands::Sync(args) = cli.command.expect("command") else {
            panic!("expected sync command");
        };
        let Some(SyncSubcommand::Resolve(args)) = args.subcommand else {
            panic!("expected sync resolve subcommand");
        };
        assert!(args.take_local);
    }
}
