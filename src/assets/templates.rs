pub const TASK_TEMPLATE: &str = include_str!("../../templates/task.md");
pub const BUG_TEMPLATE: &str = include_str!("../../templates/bug.md");
pub const FEATURE_TEMPLATE: &str = include_str!("../../templates/feature.md");
pub const WEEKLY_REVIEW_TEMPLATE: &str = include_str!("../../templates/weekly-review.md");

pub fn embedded_templates() -> [(&'static str, &'static str); 4] {
    [
        ("task.md", TASK_TEMPLATE),
        ("bug.md", BUG_TEMPLATE),
        ("feature.md", FEATURE_TEMPLATE),
        ("weekly-review.md", WEEKLY_REVIEW_TEMPLATE),
    ]
}
