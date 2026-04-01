use chrono::{DateTime, NaiveDate, Utc};

/// Search filters that can be applied to any query.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub role: Option<String>,
    pub tool: Option<String>,
    pub project: Option<String>,
    pub content_type: Option<String>,
    pub file_path: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

/// Parse a date string like "2026-03-01" into a DateTime<Utc>.
pub fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|dt| dt.and_local_timezone(Utc).single())
}
