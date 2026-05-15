use crate::adapters::git::CliGit;
use crate::cli::{NewArgs, RecurArgs, RecurNewArgs, RecurSubcommand};
use crate::config::{
    ConfigScope, config_mutate_scoped, default_write_scope, load_effective_config, load_layer,
    parse_priority, parse_state,
};
use crate::error::RiptaskError;
use crate::models::{RecurrenceFrequency, RecurringDef};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::issue_service::IssueService;
use crate::services::recurrence::{expand_tokens, instance_exists, is_due, update_last_run};
use crate::storage::issue_store;
use camino::Utf8Path;
use chrono::{NaiveDate, Utc};
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, presets::NOTHING};
use std::io::IsTerminal;

pub fn run(paths: &AppPaths, args: RecurArgs) -> Result<(), RiptaskError> {
    let cwd = crate::paths::current_cwd();
    match args.subcommand.unwrap_or(RecurSubcommand::List) {
        RecurSubcommand::List => {
            paths.require_initialized(&cwd)?;
            list(paths)
        }
        RecurSubcommand::New(args) => {
            paths.require_initialized(&cwd)?;
            new_recur(paths, *args)
        }
        RecurSubcommand::Run { date } => {
            paths.require_shared_layer()?;
            run_due(paths, date)
        }
        RecurSubcommand::Skip { recur_id } => {
            paths.require_shared_layer()?;
            skip(paths, recur_id)
        }
    }
}

fn list(paths: &AppPaths) -> Result<(), RiptaskError> {
    let config = load_effective_config(paths, &current_cwd())?;
    if std::io::stdout().is_terminal() {
        let mut table = Table::new();
        table
            .load_preset(NOTHING)
            .set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            Cell::new("ID")
                .add_attribute(Attribute::Bold)
                .add_attribute(Attribute::Dim),
            Cell::new("Frequency")
                .add_attribute(Attribute::Bold)
                .add_attribute(Attribute::Dim),
            Cell::new("Title Pattern")
                .add_attribute(Attribute::Bold)
                .add_attribute(Attribute::Dim),
            Cell::new("Last Run")
                .add_attribute(Attribute::Bold)
                .add_attribute(Attribute::Dim),
        ]);
        for def in &config.recurring {
            table.add_row(vec![
                Cell::new(&def.id).fg(Color::Cyan),
                Cell::new(def.frequency.as_str()),
                Cell::new(&def.title_pattern),
                Cell::new(def.last_run.as_deref().unwrap_or("-")).fg(Color::Grey),
            ]);
        }
        println!("{table}");
    } else {
        for def in &config.recurring {
            println!(
                "{}\t{}\t{}",
                def.id,
                def.frequency.as_str(),
                def.title_pattern
            );
        }
    }
    Ok(())
}

fn run_due(paths: &AppPaths, date: Option<String>) -> Result<(), RiptaskError> {
    let cwd = current_cwd();
    let mut config = load_effective_config(paths, &cwd)?;
    let today = date
        .as_deref()
        .map(str::parse::<NaiveDate>)
        .transpose()
        .map_err(|error| RiptaskError::Other(anyhow::Error::new(error)))?
        .unwrap_or_else(|| Utc::now().date_naive());

    let existing_issues = issue_store::list_issues(paths)?;
    let existing_docs: Vec<_> = existing_issues
        .iter()
        .filter_map(
            |p| match crate::storage::frontmatter::try_load_issue(p.as_std_path()) {
                crate::storage::frontmatter::IssueLoadResult::Ok(doc) => Some(*doc),
                _ => None,
            },
        )
        .collect();

    let mut changed_paths = Vec::new();
    let mut last_issue = None;
    let mut recurrence_writes: Vec<(ConfigScope, camino::Utf8PathBuf)> = Vec::new();
    let candidates: Vec<RecurringDef> = config.recurring.clone();
    for definition in candidates {
        if !is_due(&definition, today).map_err(RiptaskError::Other)? {
            continue;
        }
        let expanded_title = expand_tokens(&definition.title_pattern, today);

        // Dedup: skip if an issue with the same recurring ID and expanded title already exists
        if instance_exists(&existing_docs, &definition.id, &expanded_title) {
            continue;
        }

        // Create the recurring issue
        let service = IssueService::new(paths, &config);
        let new_args = NewArgs {
            title: Some(expanded_title.clone()),
            description: None,
            ai: false,
            ai_prompt: None,
            project: definition.project.clone(),
            board: definition.board.clone(),
            status: definition.status.as_ref().map(|s| s.as_str().to_owned()),
            priority: definition.priority.as_ref().map(|p| p.as_str().to_owned()),
            template: Some(
                definition
                    .template
                    .strip_prefix("templates/")
                    .unwrap_or(&definition.template)
                    .strip_suffix(".md")
                    .unwrap_or(&definition.template)
                    .to_owned(),
            ),
            edit: false,
        };
        let draft = service.prepare_issue_draft(new_args)?;
        let mut issue = service.build_local_issue_document(&draft)?;

        // Patch recurring field and definition-specific metadata
        issue.frontmatter.recurring = Some(definition.id.clone());
        if !definition.labels.is_empty() {
            issue.frontmatter.labels = definition.labels.clone();
        }
        if let Some(ref assignee) = definition.assignee {
            issue.frontmatter.assignees = vec![assignee.clone()];
        }
        if let Some(ref org) = definition.org {
            issue.frontmatter.org = Some(org.clone());
        }
        let path = paths
            .issues_dir()
            .join(format!("{}.md", issue.frontmatter.id));
        service.persist_issue(&issue)?;
        changed_paths.push(path.clone());
        last_issue = Some((
            issue.frontmatter.id.clone(),
            issue.frontmatter.title.clone(),
        ));

        println!("{}", issue.frontmatter.id);

        let (scope, _) =
            recurring_resolving_layer(paths, &cwd, &definition.id)?.ok_or_else(|| {
                RiptaskError::Config(format!(
                    "recurrence '{}' has no resolving config layer",
                    definition.id
                ))
            })?;
        let id = definition.id.clone();
        let receipt = config_mutate_scoped(paths, &cwd, scope, |partial| {
            let recurring = partial
                .recurring
                .iter_mut()
                .find(|candidate| candidate.id == id)
                .ok_or_else(|| RiptaskError::Config(format!("unknown recurrence id: {id}")))?;
            recurring.last_run = Some(today.to_string());
            Ok(())
        })?;
        update_last_run(&mut config, &definition.id, today);
        changed_paths.push(receipt.path.clone());
        recurrence_writes.push((receipt.scope, receipt.path));
    }
    for (scope, path, count) in summarize_recurrence_writes(recurrence_writes) {
        crate::ui::success(&format!(
            "wrote {} recurrence(s) to {} ({})",
            count,
            scope.label(),
            path
        ));
    }
    if let Some((id, title)) = last_issue {
        let files = changed_paths
            .iter()
            .map(|path| path.as_std_path())
            .collect::<Vec<_>>();
        maybe_auto_commit(
            &config,
            &CliGit::new(),
            paths.riptask_repo.as_std_path(),
            &format!("riptask: recur run {} - {}", id, title),
            &files,
        )?;
    }
    Ok(())
}

fn skip(paths: &AppPaths, recur_id: Option<String>) -> Result<(), RiptaskError> {
    let recur_id = recur_id.ok_or_else(|| RiptaskError::General("missing recurrence id".into()))?;
    let cwd = current_cwd();
    let effective = load_effective_config(paths, &cwd)?;
    if !effective
        .recurring
        .iter()
        .any(|definition| definition.id == recur_id)
    {
        return Err(RiptaskError::Config(format!(
            "unknown recurrence id: {recur_id}"
        )));
    }
    let (scope, _) = recurring_resolving_layer(paths, &cwd, &recur_id)?
        .ok_or_else(|| RiptaskError::Config(format!("unknown recurrence id: {recur_id}")))?;
    let today = Utc::now().date_naive();
    let receipt = config_mutate_scoped(paths, &cwd, scope, |partial| {
        let recurring = partial
            .recurring
            .iter_mut()
            .find(|definition| definition.id == recur_id)
            .ok_or_else(|| RiptaskError::Config(format!("unknown recurrence id: {recur_id}")))?;
        recurring.last_run = Some(today.to_string());
        Ok(())
    })?;
    crate::ui::success(&format!(
        "wrote recurrence to {} ({})",
        receipt.scope.label(),
        receipt.path
    ));
    Ok(())
}

/// Returns the highest-precedence layer that defines `id`, or `None` if no
/// layer defines it.
fn recurring_resolving_layer(
    paths: &AppPaths,
    cwd: &Utf8Path,
    id: &str,
) -> Result<Option<(ConfigScope, camino::Utf8PathBuf)>, RiptaskError> {
    for scope in [ConfigScope::Local, ConfigScope::User, ConfigScope::System] {
        let path = match scope.resolve(paths, cwd) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if let Some(partial) = load_layer(path.as_std_path())?
            && partial.recurring.iter().any(|recurring| recurring.id == id)
        {
            return Ok(Some((scope, path)));
        }
    }
    Ok(None)
}

fn summarize_recurrence_writes(
    writes: Vec<(ConfigScope, camino::Utf8PathBuf)>,
) -> Vec<(ConfigScope, camino::Utf8PathBuf, usize)> {
    let mut out = Vec::new();
    for (scope, path) in writes {
        if let Some((_, _, count)) = out.iter_mut().find(|(existing_scope, existing_path, _)| {
            *existing_scope == scope && *existing_path == path
        }) {
            *count += 1;
        } else {
            out.push((scope, path, 1));
        }
    }
    out
}

fn new_recur(paths: &AppPaths, args: RecurNewArgs) -> Result<(), RiptaskError> {
    let cwd = current_cwd();
    let config = load_effective_config(paths, &cwd)?;
    if config
        .recurring
        .iter()
        .any(|definition| definition.id == args.id)
    {
        return Err(RiptaskError::Config(format!(
            "duplicate recurrence id: {}",
            args.id
        )));
    }

    let frequency = match args.frequency.as_str() {
        "daily" => RecurrenceFrequency::Daily,
        "weekly" => RecurrenceFrequency::Weekly,
        "monthly" => RecurrenceFrequency::Monthly,
        "yearly" => RecurrenceFrequency::Yearly,
        _ => {
            return Err(RiptaskError::Config(format!(
                "invalid frequency: {}",
                args.frequency
            )));
        }
    };

    let status = args
        .status
        .as_deref()
        .map(parse_state)
        .transpose()
        .map_err(RiptaskError::Other)?;
    let priority = args
        .priority
        .as_deref()
        .map(parse_priority)
        .transpose()
        .map_err(RiptaskError::Other)?;

    if let Some(start) = args.start.as_deref() {
        start
            .parse::<NaiveDate>()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
    }
    if let Some(end) = args.end.as_deref() {
        end.parse::<NaiveDate>()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
    }
    if matches!(frequency, RecurrenceFrequency::Weekly) && args.day_of_week.is_none() {
        return Err(RiptaskError::Config(
            "day_of_week is required for weekly recurrences".into(),
        ));
    }
    if let Some(ref day) = args.day_of_week {
        let valid = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ];
        if !valid.contains(&day.to_lowercase().as_str()) {
            return Err(RiptaskError::Config(format!(
                "invalid day_of_week: {day} (expected: monday-sunday)"
            )));
        }
    }
    if matches!(frequency, RecurrenceFrequency::Monthly) && args.day_of_month.is_none() {
        return Err(RiptaskError::Config(
            "day_of_month is required for monthly recurrences".into(),
        ));
    }
    if let Some(day) = args.day_of_month
        && !(1..=31).contains(&day)
    {
        return Err(RiptaskError::Config(format!(
            "day_of_month must be 1-31, got {day}"
        )));
    }

    let definition = RecurringDef {
        id: args.id.clone(),
        template: args.template.unwrap_or_else(|| "task".into()),
        title_pattern: args.title_pattern,
        board: args.board,
        project: args.project,
        org: args.org,
        status,
        priority,
        assignee: args.assignee,
        labels: args.labels,
        frequency,
        day_of_week: args.day_of_week.map(|d| d.to_lowercase()),
        day_of_month: args.day_of_month.map(|value| value as u8),
        start: Some(
            args.start
                .unwrap_or_else(|| Utc::now().date_naive().to_string()),
        ),
        end: args.end,
        last_run: None,
    };
    let scope = args
        .scope
        .scope()
        .unwrap_or_else(|| default_write_scope(paths, &cwd));
    let receipt = config_mutate_scoped(paths, &cwd, scope, |partial| {
        partial.recurring.push(definition);
        Ok(())
    })?;
    crate::ui::success(&format!(
        "wrote recurrence to {} ({})",
        receipt.scope.label(),
        receipt.path
    ));
    Ok(())
}

fn current_cwd() -> camino::Utf8PathBuf {
    camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    )
}
