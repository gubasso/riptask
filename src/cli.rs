use crate::adapters::backend::MergeMethod;
use crate::config::ConfigScope;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

/// Maps each top-level subcommand to a help-output section. The order here
/// drives the section order and the order of commands within each section.
/// Command descriptions are NOT duplicated here — they are pulled at runtime
/// from each variant's doc-comment via `clap::Command::get_about()`.
pub(crate) const HELP_GROUPS: &[(&str, &[&str])] = &[
    (
        "Issue Management",
        &[
            "close", "edit", "ls", "new", "path", "recur", "reopen", "rm", "show", "status",
        ],
    ),
    (
        "Branching & Work Clones",
        &["branch", "commit", "id", "pr", "session"],
    ),
    ("Workflows", &["clone", "done", "start", "unclone"]),
    (
        "Views & Boards",
        &["board", "reorder", "reorder-down", "reorder-up", "view"],
    ),
    ("Sync & Backend", &["register", "store", "sync"]),
    ("AI Assistance", &["ask", "summarize"]),
    (
        "Setup & Meta",
        &["config", "doctor", "help", "init", "template", "version"],
    ),
];

/// Build a `clap::Command` with the grouped root help template applied.
/// All call sites that need the root command (parsing, `tsk help`, completions)
/// should use this so the rendered help is consistent.
pub fn root_command() -> clap::Command {
    let cmd = Cli::command();
    let template = build_root_help_template(&cmd);
    cmd.help_template(template)
}

fn build_root_help_template(cmd: &clap::Command) -> String {
    // Width of the name column = longest visible name + 2 spaces of padding.
    let max_name = HELP_GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);
    let pad = max_name + 2;

    let mut body = String::new();
    body.push_str("{about-with-newline}");
    body.push_str("Usage: {usage}\n\n");
    for (heading, names) in HELP_GROUPS {
        body.push_str(heading);
        body.push_str(":\n");
        let mut names: Vec<&str> = names.to_vec();
        names.sort_unstable();
        for name in &names {
            let about = cmd
                .find_subcommand(name)
                .and_then(|s| s.get_about())
                .map(|a| a.to_string())
                .unwrap_or_default();
            body.push_str("  ");
            body.push_str(name);
            for _ in name.len()..pad {
                body.push(' ');
            }
            body.push_str(&about);
            body.push('\n');
        }
        body.push('\n');
    }
    body.push_str("Options:\n{options}\n");
    body
}

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
    /// Limit to specific RepoProjects
    #[arg(long = "project", short = 'p')]
    pub projects: Vec<String>,
    /// Run across all registered RepoProjects
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
    /// Close an issue
    Close(IdArgs),
    /// Edit an existing issue in your editor
    Edit(IdArgs),
    /// List issues with optional filters
    Ls(LsArgs),
    /// Create a new issue
    New(NewArgs),
    /// Print the issue id associated with the current git branch
    Id,
    /// Print the file path of an issue
    Path(IdArgs),
    /// Manage recurring issue schedules
    Recur(RecurArgs),
    /// Reopen a closed issue
    Reopen(IdArgs),
    /// Remove an issue permanently
    Rm(IdArgs),
    /// Display issue details
    Show(IdArgs),
    /// Change the status of an issue
    #[command(name = "status")]
    Status(StatusArgs),
    /// Create, switch to, adopt, or delete an issue branch
    ///
    /// Default (no flag): creates (or checks out) the canonical branch for
    /// the given issue on the remote, then stamps frontmatter.branch and
    /// id_slug on the issue.
    ///
    /// --adopt: link the currently checked-out git branch to an issue
    /// without touching the remote. Use this when a branch was created
    /// outside of `tsk` (e.g. `git checkout -b`) and you want bare
    /// commands like `tsk edit` or `tsk show` to auto-resolve to this
    /// issue while on that branch.
    ///
    /// `tsk branch --adopt` infers the issue id from the leading
    /// `<number>-` in the current branch name.
    ///
    /// `tsk branch --adopt <id>` links to the given issue.
    ///
    /// Refuses protected branches (main/master/develop/…); refuses
    /// branches already linked to another issue; prompts before
    /// overwriting an existing link on the target issue (use -y to
    /// skip).
    ///
    /// -d / -D: delete the branch (safe / force).
    Branch(BranchArgs),
    /// Create a work-clone for an issue
    Clone(CloneArgs),
    /// Create an AI-assisted git commit
    Commit(CommitArgs),
    /// Complete an issue: merge PR, delete branch, close issue
    Done(DoneArgs),
    /// Create, edit, or show a pull/merge request
    Pr(PrArgs),
    /// Start or end a work session
    Session(SessionArgs),
    /// Start work on an issue (creates branch and draft PR)
    Start(StartArgs),
    /// Push changes and remove the current work-clone
    Unclone(UncloneArgs),
    /// Show or manage boards
    Board(BoardArgs),
    /// Set the order of issues by status
    Reorder(ReorderArgs),
    /// Move an issue one position down in its status
    #[command(name = "reorder-down")]
    ReorderDown(IdArgs),
    /// Move an issue one position up in its status
    #[command(name = "reorder-up")]
    ReorderUp(IdArgs),
    /// Regenerate board views
    View,
    /// Register a backend project (GitHub/GitLab)
    Register(RegisterArgs),
    /// Manage the task store (commits, hooks)
    Store(StoreArgs),
    /// Sync issues with a remote backend (GitHub/GitLab)
    Sync(SyncArgs),
    /// Ask an AI question about your issues
    Ask(AskArgs),
    /// Generate an AI summary of issues
    Summarize(SummarizeArgs),
    /// View or update tsk configuration
    Config(ConfigArgs),
    /// Print diagnostics: paths, config layers, and effective config
    Doctor,
    /// Generate shell completions
    #[command(hide = true)]
    Completions {
        /// Target shell
        shell: CompletionShell,
    },
    /// Show help for a command
    Help {
        /// Command name
        command: Option<String>,
    },
    /// Initialize a config layer. Does not register a project; run `tsk register` after init.
    Init(InitArgs),
    /// Manage issue templates
    Template(TemplateArgs),
    /// Print version information
    Version,
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

    /// Link the currently checked-out git branch to an issue without
    /// touching the remote. If [ID] is omitted, the issue number is
    /// inferred from the leading `<number>-` of the branch name. Refuses
    /// protected branches and branches already linked to another issue;
    /// prompts before overwriting an existing link (use -y to skip).
    #[arg(long = "adopt", conflicts_with_all = ["delete", "force_delete", "pick"])]
    pub adopt: bool,

    /// Skip confirmation prompts (for -d/-D and --adopt overwrite)
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct InitArgs {
    #[command(flatten)]
    pub scope: ScopeFlags,
    /// Overwrite an existing complete config at the chosen scope
    #[arg(long)]
    pub force: bool,
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
    /// Disable AI-assisted description generation
    #[arg(long)]
    pub no_ai: bool,
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
    #[command(flatten)]
    pub scope: ScopeArgs,

    /// Pick an existing issue interactively
    #[arg(short = 'i', long = "pick", conflicts_with_all = ["title", "title_pos"])]
    pub pick: bool,

    /// Issue title OR numeric issue ID (positional)
    #[arg(conflicts_with = "title")]
    pub title_pos: Option<String>,
    /// Issue title (or enter interactively)
    #[arg(short = 't', long)]
    pub title: Option<String>,
    /// Board to assign (new issues only)
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Initial status (new issues only)
    #[arg(short = 's', long)]
    pub status: Option<String>,
    /// Priority level (new issues only)
    #[arg(short = 'P', long)]
    pub priority: Option<String>,
    /// Template to use (new issues only)
    #[arg(short = 'T', long)]
    pub template: Option<String>,
    /// Open in $EDITOR after creation (new issues only)
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
    /// Issue title (skip AI title generation)
    #[arg(short = 't', long)]
    pub title: Option<String>,
    /// Issue description (skip AI body generation)
    #[arg(short = 'd', long)]
    pub description: Option<String>,
    /// Force AI helper to fill missing fields (e.g. description) when --title is set
    #[arg(long)]
    pub ai: bool,
    /// Target RepoProject
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
    /// Disable AI message generation
    #[arg(long)]
    pub no_ai: bool,
    /// Open commit message in editor before committing
    #[arg(short = 'e', long)]
    pub edit: bool,
    /// Provide commit message directly (skip AI)
    #[arg(short = 'm', long)]
    pub message: Option<String>,
    /// Accept AI-generated commit message without confirmation
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Stage all changes before committing (like git commit -a)
    #[arg(short = 'a', long)]
    pub all: bool,
}

#[derive(Debug, Clone, Args)]
pub struct StoreArgs {
    #[command(subcommand)]
    pub subcommand: StoreSubcommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum StoreSubcommand {
    /// Commit tsk issue changes to the task store
    Commit(StoreCommitArgs),
    /// Manage git hooks for the task store
    Hooks(HooksArgs),
}

#[derive(Debug, Clone, Args, Default)]
pub struct StoreCommitArgs {
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
    #[command(flatten)]
    pub scope: ScopeFlags,
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
    #[command(flatten)]
    pub scope: ScopeFlags,
    /// List registered RepoProjects
    #[arg(long)]
    pub list: bool,
    /// Override the auto-derived repo-project label (used to partition a shared Jira project)
    #[arg(long = "repo-project-label", alias = "project-label")]
    pub repo_project_label: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SummarizeArgs {
    /// Limit summary to a specific cycle
    #[arg(short = 'c', long)]
    pub cycle: Option<String>,
    /// Limit summary to a specific board
    #[arg(short = 'b', long)]
    pub board: Option<String>,
    /// Limit summary to a specific RepoProject
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
        #[command(flatten)]
        scope: ScopeFlags,
    },
    /// Edit a configuration layer
    Edit {
        #[command(flatten)]
        scope: ScopeFlags,
    },
}

#[derive(Debug, Clone, Args, Default)]
#[group(multiple = false)]
pub struct ScopeFlags {
    #[arg(long)]
    pub system: bool,
    #[arg(long = "user", alias = "global")]
    pub user: bool,
    #[arg(long)]
    pub local: bool,
}

impl ScopeFlags {
    pub fn scope(&self) -> Option<ConfigScope> {
        if self.system {
            Some(ConfigScope::System)
        } else if self.user {
            Some(ConfigScope::User)
        } else if self.local {
            Some(ConfigScope::Local)
        } else {
            None
        }
    }
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
    use super::{
        Cli, Commands, ConfigSubcommand, HELP_GROUPS, PrSubcommand, SessionSubcommand,
        SyncSubcommand,
    };
    use crate::config::ConfigScope;
    use clap::{CommandFactory, Parser};

    /// Guards against drift between `HELP_GROUPS` and the actual `Commands`
    /// enum. Every visible top-level subcommand must appear in `HELP_GROUPS`
    /// exactly once and be rendered in `tsk --help`; every entry in
    /// `HELP_GROUPS` must correspond to a real visible subcommand.
    #[test]
    fn root_help_template_matches_visible_subcommands() {
        let visible: Vec<String> = Cli::command()
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .map(|s| s.get_name().to_string())
            .collect();
        let mapped: Vec<&str> = HELP_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();

        for name in &visible {
            let count = mapped.iter().filter(|m| **m == name.as_str()).count();
            assert_eq!(
                count, 1,
                "subcommand `{name}` should appear exactly once in HELP_GROUPS (found {count})"
            );
        }
        for entry in &mapped {
            assert!(
                visible.iter().any(|v| v == entry),
                "HELP_GROUPS contains `{entry}` which is not a visible subcommand"
            );
        }

        let rendered = super::root_command().render_help().to_string();
        for name in &visible {
            let prefix = format!("  {name} ");
            assert!(
                rendered.lines().any(|line| line.starts_with(&prefix)),
                "tsk --help did not render `{name}` — check build_root_help_template"
            );
        }
    }

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
    fn pr_shorthand_no_ai_parses() {
        let cli = Cli::try_parse_from(["tsk", "pr", "--no-ai", "42"]).expect("parse");
        let Commands::Pr(args) = cli.command.expect("command") else {
            panic!("expected pr command");
        };
        assert_eq!(args.id.as_deref(), Some("42"));
        assert!(args.no_ai);
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
    }

    #[test]
    fn new_description_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "-t", "hello", "-d", "desc"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert_eq!(args.title.as_deref(), Some("hello"));
        assert_eq!(args.description.as_deref(), Some("desc"));
    }

    #[test]
    fn new_short_description_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "-d", "desc"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert_eq!(args.description.as_deref(), Some("desc"));
        assert!(args.title.is_none());
    }

    #[test]
    fn new_ai_flag_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "--ai"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert!(args.ai);
        assert!(args.title.is_none());
    }

    #[test]
    fn new_title_with_ai_flag_parses() {
        let cli = Cli::try_parse_from(["tsk", "new", "-t", "hello", "--ai"]).expect("parse");
        let Commands::New(args) = cli.command.expect("command") else {
            panic!("expected new command");
        };
        assert_eq!(args.title.as_deref(), Some("hello"));
        assert!(args.ai);
    }

    #[test]
    fn new_positional_rejected() {
        let err = Cli::try_parse_from(["tsk", "new", "hello"]).expect_err("reject positional");
        let rendered = err.to_string();
        assert!(rendered.contains("hello"));
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
    fn start_edit_flag_parses() {
        let cli = Cli::try_parse_from(["tsk", "start", "-e", "my title"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert_eq!(args.title_pos.as_deref(), Some("my title"));
        assert!(args.edit);
    }

    #[test]
    fn start_pick_parses() {
        let cli = Cli::try_parse_from(["tsk", "start", "--pick"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert!(args.pick);
    }

    #[test]
    fn start_project_scope_parses() {
        let cli =
            Cli::try_parse_from(["tsk", "start", "-p", "myproject", "--pick"]).expect("parse");
        let Commands::Start(args) = cli.command.expect("command") else {
            panic!("expected start command");
        };
        assert_eq!(args.scope.projects, vec!["myproject".to_string()]);
        assert!(args.pick);
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

    #[test]
    fn config_set_user_scope_parses() {
        let cli = Cli::try_parse_from(["tsk", "config", "set", "--user", "auto_commit", "true"])
            .expect("parse");
        let Commands::Config(args) = cli.command.expect("command") else {
            panic!("expected config command");
        };
        let Some(ConfigSubcommand::Set { scope, .. }) = args.subcommand else {
            panic!("expected config set");
        };
        assert_eq!(scope.scope(), Some(ConfigScope::User));
    }

    #[test]
    fn config_set_global_alias_parses() {
        let cli = Cli::try_parse_from(["tsk", "config", "set", "--global", "auto_commit", "true"])
            .expect("parse");
        let Commands::Config(args) = cli.command.expect("command") else {
            panic!("expected config command");
        };
        let Some(ConfigSubcommand::Set { scope, .. }) = args.subcommand else {
            panic!("expected config set");
        };
        assert_eq!(scope.scope(), Some(ConfigScope::User));
    }
}
