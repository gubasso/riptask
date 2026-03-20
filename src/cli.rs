use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "tsk",
    version,
    about = "Plaintext issue tracker",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ScopeArgs {
    #[arg(long = "project", short = 'p')]
    pub projects: Vec<String>,
    #[arg(long = "all-projects", short = 'A')]
    pub all_projects: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct IdArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    pub id: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Init,
    New(NewArgs),
    Edit(IdArgs),
    Show(IdArgs),
    #[command(name = "move")]
    Move(MoveArgs),
    Close(IdArgs),
    Reopen(IdArgs),
    Rm(IdArgs),
    Ls(LsArgs),
    Path(IdArgs),
    Branch(IdArgs),
    Pr(IdArgs),
    Board(BoardArgs),
    View,
    Reorder(ReorderArgs),
    #[command(name = "reorder-up")]
    ReorderUp(IdArgs),
    #[command(name = "reorder-down")]
    ReorderDown(IdArgs),
    Sync(SyncArgs),
    Push(PushArgs),
    Resolve(ResolveArgs),
    Session(SessionArgs),
    Commit(CommitArgs),
    Recur(RecurArgs),
    Template(TemplateArgs),
    Register(RegisterArgs),
    Summarize(SummarizeArgs),
    Ask(AskArgs),
    Config(ConfigArgs),
    Hooks(HooksArgs),
    #[command(hide = true)]
    Completions {
        shell: CompletionShell,
    },
    Version,
    Help {
        command: Option<String>,
    },
}

#[derive(Debug, Clone, Args, Default)]
pub struct NewArgs {
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub board: Option<String>,
    #[arg(long)]
    pub state: Option<String>,
    #[arg(long)]
    pub priority: Option<String>,
    #[arg(long, short = 't')]
    pub template: Option<String>,
    #[arg(long)]
    pub ai: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct MoveArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    pub id: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct LsArgs {
    #[arg(long)]
    pub state: Option<String>,
    #[arg(long)]
    pub priority: Option<String>,
    #[arg(long = "project")]
    pub projects: Vec<String>,
    #[arg(long = "all-projects", short = 'A')]
    pub all_projects: bool,
    #[arg(long)]
    pub board: Option<String>,
    #[arg(long)]
    pub cycle: Option<String>,
    #[arg(long)]
    pub assignee: Option<String>,
    #[arg(long = "all")]
    pub include_done: bool,
    #[arg(long)]
    pub conflicts: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct BoardArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub board: Option<String>,
    #[arg(long)]
    pub open: bool,
    #[arg(long)]
    pub path: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ReorderArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    #[arg(long)]
    pub board: Option<String>,
    pub state: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub subcommand: Option<SyncSubcommand>,
    #[arg(long)]
    pub all: bool,
    #[arg(long = "remote")]
    pub remote: Option<String>,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub triage: bool,
    #[arg(long = "auto-triage")]
    pub auto_triage: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SyncSubcommand {
    Pull,
    Push,
    Status,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PushArgs {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long = "all-projects", short = 'A')]
    pub all_projects: bool,
    #[arg(long)]
    pub all: bool,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ResolveArgs {
    pub id: String,
    #[arg(long = "take-remote")]
    pub take_remote: bool,
    #[arg(long = "take-local")]
    pub take_local: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub subcommand: SessionSubcommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SessionSubcommand {
    Start { id: Option<String> },
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
    List,
    New(Box<RecurNewArgs>),
    Run { date: Option<String> },
    Skip { recur_id: Option<String> },
}

#[derive(Debug, Clone, Args, Default)]
pub struct RecurNewArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub template: Option<String>,
    #[arg(long)]
    pub title_pattern: String,
    #[arg(long)]
    pub frequency: String,
    #[arg(long)]
    pub board: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub org: Option<String>,
    #[arg(long)]
    pub state: Option<String>,
    #[arg(long)]
    pub priority: Option<String>,
    #[arg(long)]
    pub assignee: Option<String>,
    #[arg(long)]
    pub labels: Vec<String>,
    #[arg(long)]
    pub day_of_week: Option<String>,
    #[arg(long)]
    pub day_of_month: Option<u32>,
    #[arg(long)]
    pub start: Option<String>,
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
    List,
    Show { name: Option<String> },
    New { name: Option<String> },
    Edit { name: Option<String> },
    Rm { name: Option<String> },
    Validate { name: Option<String> },
}

#[derive(Debug, Clone, Args, Default)]
pub struct RegisterArgs {
    #[arg(long)]
    pub list: bool,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SummarizeArgs {
    #[arg(long)]
    pub cycle: Option<String>,
    #[arg(long)]
    pub board: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct AskArgs {
    pub question: String,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub subcommand: Option<ConfigSubcommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigSubcommand {
    Set { key: String, value: String },
}

#[derive(Debug, Clone, Args, Default)]
pub struct HooksArgs {
    #[command(subcommand)]
    pub subcommand: Option<HooksSubcommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum HooksSubcommand {
    Install {
        #[arg(long)]
        force: bool,
    },
    Update {
        #[arg(long)]
        force: bool,
    },
    Status,
    Uninstall {
        #[arg(long)]
        force: bool,
    },
}
