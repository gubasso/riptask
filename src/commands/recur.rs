use crate::adapters::git::CliGit;
use crate::cli::{NewArgs, RecurArgs, RecurNewArgs, RecurSubcommand};
use crate::config::{load_config, parse_priority, parse_state, save_config};
use crate::error::RiptskError;
use crate::models::{RecurrenceFrequency, RecurringDef};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::issue_service::IssueService;
use crate::services::recurrence::{expand_tokens, instance_exists, is_due, update_last_run};
use crate::storage::issue_store;
use chrono::{NaiveDate, Utc};
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, presets::NOTHING};
use std::io::IsTerminal;

pub fn run(paths: &AppPaths, args: RecurArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    match args.subcommand.unwrap_or(RecurSubcommand::List) {
        RecurSubcommand::List => list(paths),
        RecurSubcommand::New(args) => new_recur(paths, *args),
        RecurSubcommand::Run { date } => run_due(paths, date),
        RecurSubcommand::Skip { recur_id } => skip(paths, recur_id),
    }
}

fn list(paths: &AppPaths) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path())?;
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

fn run_due(paths: &AppPaths, date: Option<String>) -> Result<(), RiptskError> {
    let mut config = load_config(paths.config_path().as_std_path())?;
    let today = date
        .as_deref()
        .map(str::parse::<NaiveDate>)
        .transpose()
        .map_err(|error| RiptskError::Other(anyhow::Error::new(error)))?
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

    let mut changed_paths = vec![paths.config_path()];
    let mut last_issue = None;
    for definition in config.recurring.clone() {
        if !is_due(&definition, today).map_err(RiptskError::Other)? {
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
            title_pos: None,
            title: Some(expanded_title.clone()),
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
            ai: false,
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

        update_last_run(&mut config, &definition.id, today);
    }
    save_config(paths.config_path().as_std_path(), &config)?;
    if let Some((id, title)) = last_issue {
        let files = changed_paths
            .iter()
            .map(|path| path.as_std_path())
            .collect::<Vec<_>>();
        maybe_auto_commit(
            &config,
            &CliGit::new(),
            paths.riptsk_repo.as_std_path(),
            &format!("riptsk: recur run {} - {}", id, title),
            &files,
        )?;
    }
    Ok(())
}

fn skip(paths: &AppPaths, recur_id: Option<String>) -> Result<(), RiptskError> {
    let recur_id = recur_id.ok_or_else(|| RiptskError::General("missing recurrence id".into()))?;
    let mut config = load_config(paths.config_path().as_std_path())?;
    let today = Utc::now().date_naive();
    update_last_run(&mut config, &recur_id, today);
    save_config(paths.config_path().as_std_path(), &config)
}

fn new_recur(paths: &AppPaths, args: RecurNewArgs) -> Result<(), RiptskError> {
    let mut config = load_config(paths.config_path().as_std_path())?;
    if config
        .recurring
        .iter()
        .any(|definition| definition.id == args.id)
    {
        return Err(RiptskError::Config(format!(
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
            return Err(RiptskError::Config(format!(
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
        .map_err(RiptskError::Other)?;
    let priority = args
        .priority
        .as_deref()
        .map(parse_priority)
        .transpose()
        .map_err(RiptskError::Other)?;

    if let Some(start) = args.start.as_deref() {
        start
            .parse::<NaiveDate>()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
    }
    if let Some(end) = args.end.as_deref() {
        end.parse::<NaiveDate>()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
    }
    if matches!(frequency, RecurrenceFrequency::Weekly) && args.day_of_week.is_none() {
        return Err(RiptskError::Config(
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
            return Err(RiptskError::Config(format!(
                "invalid day_of_week: {day} (expected: monday-sunday)"
            )));
        }
    }
    if matches!(frequency, RecurrenceFrequency::Monthly) && args.day_of_month.is_none() {
        return Err(RiptskError::Config(
            "day_of_month is required for monthly recurrences".into(),
        ));
    }
    if let Some(day) = args.day_of_month
        && !(1..=31).contains(&day)
    {
        return Err(RiptskError::Config(format!(
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
    config.recurring.push(definition);
    save_config(paths.config_path().as_std_path(), &config)?;
    crate::ui::success(&format!("added recurrence {}", args.id));
    Ok(())
}
