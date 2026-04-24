use anyhow::Context;
use clap::FromArgMatches;
use riptask::cli::{Cli, Commands, CompletionShell, root_command};
use riptask::commands;
use riptask::error::RiptskError;
use riptask::paths::AppPaths;
use std::fs;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                eprintln!("{} {error}", console::style("tsk:").red().bold());
            } else {
                eprintln!("tsk: {error}");
            }
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn run() -> Result<(), RiptskError> {
    let matches = root_command().get_matches();
    let cli = Cli::from_arg_matches(&matches).map_err(anyhow::Error::from)?;
    let paths = AppPaths::from_env().context("failed to resolve application paths")?;
    fs::create_dir_all(paths.log_path().parent().unwrap())
        .context("failed to create riptsk log directory")?;
    let file_appender =
        tracing_appender::rolling::never(paths.state_root.as_std_path(), "riptsk.log");
    let (non_blocking_writer, _guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_env("RIPTSK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking_writer)
                .with_ansi(false),
        )
        .try_init()
        .context("failed to initialize tracing subscriber")?;

    riptask::services::project_detection::ensure_registered(
        &paths,
        &riptask::adapters::git::CliGit::new(),
        &riptask::adapters::prompts::DialoguerPrompts,
    )?;

    match cli.command.unwrap_or(Commands::Help { command: None }) {
        Commands::Init => commands::init::run(&paths),
        Commands::Config(args) => commands::config_cmd::run(&paths, args),
        Commands::Show(args) => commands::issues::show(&paths, args),
        Commands::Path(args) => commands::issues::path(&paths, args),
        Commands::Id => commands::id::run(&paths),
        Commands::Ls(args) => commands::issues::list(&paths, args),
        Commands::Version => {
            println!("tsk {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Commands::Completions { shell } => {
            let shell = match shell {
                CompletionShell::Bash => clap_complete::Shell::Bash,
                CompletionShell::Zsh => clap_complete::Shell::Zsh,
                CompletionShell::Fish => clap_complete::Shell::Fish,
            };
            let mut root = root_command();
            clap_complete::generate(shell, &mut root, "tsk", &mut std::io::stdout());
            Ok(())
        }
        Commands::Help { command } => {
            if let Some(command) = command {
                let mut root = root_command();
                if let Some(sub) = root.find_subcommand_mut(&command) {
                    sub.print_long_help().map_err(anyhow::Error::from)?;
                    println!();
                    Ok(())
                } else {
                    Err(RiptskError::General(format!("unknown command: {command}")))
                }
            } else {
                let mut root = root_command();
                root.print_long_help().map_err(anyhow::Error::from)?;
                println!();
                Ok(())
            }
        }
        Commands::New(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::issues::new(&paths, args))
        }
        Commands::Edit(args) => commands::issues::edit(&paths, args),
        Commands::Status(args) => commands::issues::set_status(&paths, args),
        Commands::Close(args) => commands::issues::close(&paths, args),
        Commands::Reopen(args) => commands::issues::reopen(&paths, args),
        Commands::Rm(args) => commands::issues::remove(&paths, args),
        Commands::Board(args) => commands::views::board(&paths, args),
        Commands::View => commands::views::view(&paths),
        Commands::Reorder(args) => commands::views::reorder(&paths, args),
        Commands::ReorderUp(args) => commands::views::reorder_up(&paths, args),
        Commands::ReorderDown(args) => commands::views::reorder_down(&paths, args),
        Commands::Template(args) => commands::templates::run(&paths, args),
        Commands::Register(args) => commands::register::run(&paths, args),
        Commands::Recur(args) => commands::recur::run(&paths, args),
        Commands::Sync(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::sync_cmd::run(&paths, args))
        }
        Commands::Session(args) => commands::sync_cmd::session(&paths, args),
        Commands::Commit(args) => commands::commit::run(&paths, args),
        Commands::Branch(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::branch::branch(&paths, args))
        }
        Commands::Clone(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::clone_cmd::run(&paths, args))
        }
        Commands::Unclone(args) => commands::unclone::run(&paths, args),
        Commands::Done(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::done::run(&paths, args))
        }
        Commands::Start(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::start::run(&paths, args))
        }
        Commands::Pr(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::pr::run(&paths, args))
        }
        Commands::Store(args) => commands::store::run(&paths, args),
        Commands::Summarize(args) => commands::ai::summarize(&paths, args),
        Commands::Ask(args) => commands::ai::ask(&paths, args),
    }
}
