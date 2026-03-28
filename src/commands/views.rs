use crate::adapters::picker::{FzfPicker, IssueDisplayMode, Picker};
use crate::cli::{BoardArgs, IdArgs, ReorderArgs};
use crate::config::{load_config, parse_state};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::id_resolution;
use crate::services::issue_service::{IssueService, ShiftDirection};
use crate::services::view_builder::ViewBuilder;
use anyhow::Context;
use console::style;
use std::fs;
use std::io::IsTerminal;

pub fn view(paths: &AppPaths) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    ViewBuilder::new(paths, &config)
        .regenerate_all(None)
        .map_err(RiptskError::Other)
}

pub fn board(paths: &AppPaths, args: BoardArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let scope =
        crate::scope::resolve_scope(&args.scope.projects, args.scope.all_projects, &cwd, &config)?;
    let builder = ViewBuilder::new(paths, &config);
    builder
        .regenerate_all(Some(&scope))
        .map_err(RiptskError::Other)?;
    let path = builder.board_path(args.board.as_deref(), args.all);

    if args.path {
        println!("{path}");
    } else if args.open {
        let opener = config.ui.opener.as_deref().unwrap_or("xdg-open");
        let parts: Vec<&str> = opener.split_whitespace().collect();
        let (program, cmd_args) = parts
            .split_first()
            .ok_or_else(|| RiptskError::General("empty opener command".into()))?;
        std::process::Command::new(program)
            .args(cmd_args)
            .arg(path.as_str())
            .status()
            .context("failed to open board")
            .map_err(RiptskError::Other)?;
    } else {
        let max_depth = config.ui.tree_depth.unwrap_or(3) as usize;
        render_tree(path.as_str(), 0, max_depth)?;
    }
    Ok(())
}

fn render_tree(path: &str, depth: usize, max_depth: usize) -> Result<(), RiptskError> {
    let p = std::path::Path::new(path);
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    let tty = std::io::stdout().is_terminal();

    if depth == 0 {
        if tty {
            println!("{}", style(format!("{name}/")).bold());
        } else {
            println!("{name}/");
        }
        if depth < max_depth {
            let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            let count = entries.len();
            for (i, entry) in entries.iter().enumerate() {
                let is_last = i == count - 1;
                render_tree_inner(
                    &entry.path().to_string_lossy(),
                    depth + 1,
                    max_depth,
                    "",
                    is_last,
                    tty,
                )?;
            }
        }
    } else if p.is_dir() {
        if tty {
            println!("{}", style(format!("{name}/")).bold());
        } else {
            println!("{name}/");
        }
        if depth < max_depth {
            let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            let count = entries.len();
            for (i, entry) in entries.iter().enumerate() {
                let is_last = i == count - 1;
                render_tree_inner(
                    &entry.path().to_string_lossy(),
                    depth + 1,
                    max_depth,
                    "",
                    is_last,
                    tty,
                )?;
            }
        }
    } else if tty {
        println!("{}", style(&*name).dim());
    } else {
        println!("{name}");
    }
    Ok(())
}

fn render_tree_inner(
    path: &str,
    depth: usize,
    max_depth: usize,
    prefix: &str,
    is_last: bool,
    tty: bool,
) -> Result<(), RiptskError> {
    let p = std::path::Path::new(path);
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    let connector = if is_last { "└── " } else { "├── " };

    if p.is_dir() {
        if tty {
            println!("{prefix}{connector}{}", style(format!("{name}/")).bold());
        } else {
            println!("{prefix}{connector}{name}/");
        }
        if depth < max_depth {
            let continuation = if is_last { "    " } else { "│   " };
            let child_prefix = format!("{prefix}{continuation}");
            let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            let count = entries.len();
            for (i, entry) in entries.iter().enumerate() {
                let child_is_last = i == count - 1;
                render_tree_inner(
                    &entry.path().to_string_lossy(),
                    depth + 1,
                    max_depth,
                    &child_prefix,
                    child_is_last,
                    tty,
                )?;
            }
        }
    } else if tty {
        let colored_name = color_issue_file(&name);
        println!("{prefix}{connector}{colored_name}");
    } else {
        println!("{prefix}{connector}{name}");
    }
    Ok(())
}

fn color_issue_file(name: &str) -> String {
    // Issue files in board tree are under state directories (todo/, in-progress/, etc.)
    // The filename itself doesn't encode state, but we keep it dim for non-directory entries
    format!("{}", style(name).dim())
}

pub fn reorder(paths: &AppPaths, args: ReorderArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let service = IssueService::new(paths, &config);
    let board = args
        .board
        .or_else(|| config.boards.first().map(|board| board.name.clone()))
        .unwrap_or_else(|| "personal".into());
    let state = parse_state(&args.status).map_err(RiptskError::Other)?;

    let ids = if args.ids.is_empty() {
        let scope = crate::scope::resolve_scope(
            &args.scope.projects,
            args.scope.all_projects,
            &cwd,
            &config,
        )?;
        let picker = FzfPicker {
            fzf_opts: config.ui.fzf_opts.clone(),
        };
        let mode = if args.scope.all_projects {
            IssueDisplayMode::AllProjects
        } else {
            IssueDisplayMode::PerProject
        };
        let issues_dir = paths.issues_dir();
        let issues = service
            .list_matching(
                &crate::cli::LsArgs {
                    status: Some(state.as_str().to_owned()),
                    board: Some(board.clone()),
                    ..Default::default()
                },
                &scope,
            )?
            .documents
            .into_iter()
            .filter(|issue| issue.frontmatter.board == board)
            .collect::<Vec<_>>();
        picker.pick_many(&issues, "reorder> ", Some(issues_dir.as_str()), &mode)?
    } else {
        args.ids
            .into_iter()
            .map(|id| id_resolution::resolve_id(paths, &config, &cwd, &id))
            .collect::<Result<Vec<_>, _>>()?
    };

    service.reorder_lane(&board, &state, &ids)
}

pub fn reorder_up(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    shift(paths, args, ShiftDirection::Up)
}

pub fn reorder_down(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    shift(paths, args, ShiftDirection::Down)
}

fn shift(paths: &AppPaths, args: IdArgs, direction: ShiftDirection) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let service = IssueService::new(paths, &config);
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, args.id, &args.scope)? else {
        return Ok(());
    };
    service.shift_issue_order(&id, direction)
}
