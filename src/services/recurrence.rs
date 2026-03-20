use crate::config::Config;
use crate::models::{RecurrenceFrequency, RecurringDef};
use anyhow::Result;
use jiff::civil::{Date, Weekday};

pub fn expand_tokens(input: &str, date: Date) -> String {
    input
        .replace("{YYYY-MM-DD}", &date.to_string())
        .replace("{date}", &date.to_string())
        .replace("{week}", &date.iso_week_date().week().to_string())
}

pub fn is_due(def: &RecurringDef, today: Date) -> Result<bool> {
    let Some(start) = def.start.as_deref() else {
        return Ok(false);
    };
    let start = start.parse::<Date>()?;
    if today < start {
        return Ok(false);
    }
    if let Some(end) = def.end.as_deref()
        && today > end.parse::<Date>()?
    {
        return Ok(false);
    }
    let last_run = def
        .last_run
        .as_deref()
        .map(str::parse::<Date>)
        .transpose()?;
    let due = match def.frequency {
        RecurrenceFrequency::Daily => last_run.is_none_or(|last| last < today),
        RecurrenceFrequency::Weekly => {
            let target = def.day_of_week.as_deref().unwrap_or("monday");
            weekday_name(today.weekday()) == target && last_run.is_none_or(|last| last < today)
        }
        RecurrenceFrequency::Monthly => {
            today.day() == i8::try_from(def.day_of_month.unwrap_or(1)).unwrap_or(1)
                && last_run.is_none_or(|last| last < today)
        }
        RecurrenceFrequency::Yearly => {
            today.month() == start.month()
                && today.day() == start.day()
                && last_run.is_none_or(|last| last < today)
        }
    };
    Ok(due)
}

/// Check if a recurring issue instance already exists (dedup by recurring ID + expanded title)
pub fn instance_exists(
    issues: &[crate::domain::issue::IssueDocument],
    recur_id: &str,
    expanded_title: &str,
) -> bool {
    issues.iter().any(|doc| {
        doc.frontmatter.recurring.as_deref() == Some(recur_id)
            && doc.frontmatter.title == expanded_title
    })
}

pub fn update_last_run(config: &mut Config, recur_id: &str, date: Date) {
    if let Some(def) = config.recurring.iter_mut().find(|def| def.id == recur_id) {
        def.last_run = Some(date.to_string());
    }
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => "monday",
        Weekday::Tuesday => "tuesday",
        Weekday::Wednesday => "wednesday",
        Weekday::Thursday => "thursday",
        Weekday::Friday => "friday",
        Weekday::Saturday => "saturday",
        Weekday::Sunday => "sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_tokens, is_due};
    use crate::domain::issue::{IssueState, Priority};
    use crate::models::{RecurrenceFrequency, RecurringDef};
    use jiff::civil::date;

    fn sample_def() -> RecurringDef {
        RecurringDef {
            id: "weekly".into(),
            template: "templates/weekly-review.md".into(),
            title_pattern: "Weekly review - {YYYY-MM-DD}".into(),
            board: Some("personal".into()),
            project: Some("personal".into()),
            org: None,
            state: Some(IssueState::Todo),
            priority: Some(Priority::Medium),
            assignee: None,
            labels: vec!["recurring".into()],
            frequency: RecurrenceFrequency::Weekly,
            day_of_week: Some("tuesday".into()),
            day_of_month: None,
            start: Some("2026-03-17".into()),
            end: None,
            last_run: None,
        }
    }

    #[test]
    fn expands_tokens() {
        assert_eq!(
            expand_tokens("Weekly review - {YYYY-MM-DD}", date(2026, 3, 17)),
            "Weekly review - 2026-03-17"
        );
    }

    #[test]
    fn weekly_definition_is_due_on_target_day() {
        assert!(is_due(&sample_def(), date(2026, 3, 17)).expect("due"));
    }
}
