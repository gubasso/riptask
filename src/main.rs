use anyhow::Context;
use clap::CommandFactory;
use clap::Parser;
use riptsk::cli::{Cli, Commands, CompletionShell};
use riptsk::commands;
use riptsk::error::RiptskError;
use riptsk::paths::AppPaths;
use std::process::ExitCode;

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
    let cli = Cli::parse();
    let paths = AppPaths::from_env().context("failed to resolve application paths")?;

    riptsk::services::project_detection::ensure_registered(
        &paths,
        &riptsk::adapters::git::CliGit::new(),
    )?;

    match cli.command.unwrap_or(Commands::Help { command: None }) {
        Commands::Init => commands::init::run(&paths),
        Commands::Config(args) => commands::config_cmd::run(&paths, args),
        Commands::Show(args) => commands::issues::show(&paths, args),
        Commands::Path(args) => commands::issues::path(&paths, args),
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
            let mut root = Cli::command();
            clap_complete::generate(shell, &mut root, "tsk", &mut std::io::stdout());
            Ok(())
        }
        Commands::Help { command } => {
            if let Some(command) = command {
                let mut root = Cli::command();
                if let Some(sub) = root.find_subcommand_mut(&command) {
                    sub.print_long_help().map_err(anyhow::Error::from)?;
                    println!();
                    Ok(())
                } else {
                    Err(RiptskError::General(format!("unknown command: {command}")))
                }
            } else {
                let mut root = Cli::command();
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
        Commands::Commit(args) => commands::sync_cmd::commit(&paths, args),
        Commands::Branch(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::branch::branch(&paths, args))
        }
        Commands::Done(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::done::run(&paths, args))
        }
        Commands::Pr(args) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to construct tokio runtime")?;
            runtime.block_on(commands::pr::run(&paths, args))
        }
        Commands::Hooks(args) => commands::hooks::run(&paths, args),
        Commands::Summarize(args) => commands::ai::summarize(&paths, args),
        Commands::Ask(args) => commands::ai::ask(&paths, args),
    }
}
