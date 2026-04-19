use chrono::{DateTime, NaiveDate, Utc};

/// Search filters that can be applied to any query.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub role: Option<String>,
    pub tool: Option<String>,
    /// Resolved list of project directory names (as stored in the tantivy
    /// `project` field) that matched the user's `--project` hint. Empty = no
    /// project filter.
    pub project_dirs: Vec<String>,
    pub content_type: Option<String>,
    pub file_path: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

impl SearchFilters {
    /// Returns true if any filter is set (useful for deciding whether a bare
    /// `dex search` with no query string is meaningful).
    pub fn any_set(&self) -> bool {
        self.role.is_some()
            || self.tool.is_some()
            || !self.project_dirs.is_empty()
            || self.content_type.is_some()
            || self.file_path.is_some()
            || self.after.is_some()
            || self.before.is_some()
    }
}

/// Parse a date string like "2026-03-01" into a DateTime<Utc>.
pub fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|dt| dt.and_local_timezone(Utc).single())
}
