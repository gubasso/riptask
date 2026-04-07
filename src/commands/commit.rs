use crate::adapters::ai::AiBackend;
use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::CommitArgs;
use crate::commands::ai::optional_backend;
use crate::config::load_config;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use std::process::Command;

enum ConfirmChoice {
    Accept(String),
    Edit(String),
    Cancel,
}

pub fn run(paths: &AppPaths, args: CommitArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;

    let cwd = std::env::current_dir()
        .map_err(|e| RiptskError::General(format!("failed to determine current directory: {e}")))?;

    let git = CliGit::new();
    let repo = git.repo_root(&cwd)?;

    if !git.has_staged_changes(&repo)? {
        return Err(RiptskError::General("no staged changes to commit".into()));
    }

    if let Some(msg) = args.message {
        return if args.edit {
            run_git_commit_edit(&repo, &msg)
        } else {
            run_git_commit(&repo, &msg)
        };
    }

    let use_ai = config.ai.enabled && config.ai.features.commit && !args.no_ai;
    if !use_ai {
        return open_editor_commit(&repo, "");
    }

    let backend = match optional_backend(&config) {
        Some(b) => b,
        None => {
            crate::ui::warn("AI unavailable: ai.command is not configured");
            return open_editor_commit(&repo, "");
        }
    };

    let diff = git.staged_diff(&repo)?;
    if diff.is_empty() {
        return Err(RiptskError::General(
            "staged diff is empty, cannot generate commit message".into(),
        ));
    }

    crate::ui::info("Generating commit message...");
    let generated = match backend.generate_commit_message(&diff) {
        Ok(message) => {
            let trimmed = message.trim().to_owned();
            if trimmed.is_empty() {
                crate::ui::warn("AI returned an empty commit message; opening editor");
                return open_editor_commit(&repo, "");
            }
            trimmed
        }
        Err(error) => {
            crate::ui::warn(&format!("AI commit message unavailable: {error}"));
            return open_editor_commit(&repo, "");
        }
    };

    if args.edit {
        return run_git_commit_edit(&repo, &generated);
    }

    match confirm_message(&generated) {
        ConfirmChoice::Accept(msg) => run_git_commit(&repo, &msg),
        ConfirmChoice::Edit(msg) => open_editor_commit(&repo, &msg),
        ConfirmChoice::Cancel => {
            crate::ui::info("Commit cancelled.");
            Ok(())
        }
    }
}

fn confirm_message(message: &str) -> ConfirmChoice {
    println!();
    println!("{}", console::style("Generated commit message:").bold());
    println!("{}", console::style("─".repeat(60)).dim());
    println!("{message}");
    println!("{}", console::style("─".repeat(60)).dim());
    println!();

    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return ConfirmChoice::Accept(message.to_owned());
    }

    let items = &["Accept", "Edit", "Cancel"];
    let selection = dialoguer::Select::new()
        .with_prompt("What would you like to do?")
        .items(items)
        .default(0)
        .interact();

    match selection {
        Ok(0) => ConfirmChoice::Accept(message.to_owned()),
        Ok(1) => ConfirmChoice::Edit(message.to_owned()),
        _ => ConfirmChoice::Cancel,
    }
}

fn open_editor_commit(repo: &std::path::Path, seed: &str) -> Result<(), RiptskError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).arg("commit");
    if !seed.is_empty() {
        cmd.arg("--edit").arg("-m").arg(seed);
    }
    let status = cmd
        .status()
        .map_err(|e| RiptskError::General(format!("failed to run git commit: {e}")))?;
    if !status.success() {
        return Err(RiptskError::General("git commit failed".into()));
    }
    Ok(())
}

fn run_git_commit(repo: &std::path::Path, message: &str) -> Result<(), RiptskError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit", "-m", message])
        .status()
        .map_err(|e| RiptskError::General(format!("failed to run git commit: {e}")))?;
    if !status.success() {
        return Err(RiptskError::General("git commit failed".into()));
    }
    Ok(())
}

fn run_git_commit_edit(repo: &std::path::Path, message: &str) -> Result<(), RiptskError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit", "--edit", "-m", message])
        .status()
        .map_err(|e| RiptskError::General(format!("failed to run git commit: {e}")))?;
    if !status.success() {
        return Err(RiptskError::General("git commit failed".into()));
    }
    Ok(())
}
