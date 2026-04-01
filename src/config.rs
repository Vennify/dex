use directories::ProjectDirs;
use std::path::PathBuf;

/// All paths dex cares about.
pub struct Config {
    /// Where Claude Code stores conversations: ~/.claude/projects/
    pub claude_projects_dir: PathBuf,
    /// Where Claude Code stores session metadata: ~/.claude/usage-data/session-meta/
    pub claude_session_meta_dir: PathBuf,
    /// Where dex stores its index: ~/.local/share/dex/
    pub data_dir: PathBuf,
    /// Tantivy index directory
    pub tantivy_dir: PathBuf,
    /// Incremental index state file
    pub state_file: PathBuf,
}

impl Config {
    pub fn new() -> Self {
        let home = dirs_home();
        let claude_dir = home.join(".claude");

        let data_dir = ProjectDirs::from("", "", "dex")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| home.join(".local/share/dex"));

        Config {
            claude_projects_dir: claude_dir.join("projects"),
            claude_session_meta_dir: claude_dir.join("usage-data/session-meta"),
            tantivy_dir: data_dir.join("tantivy"),
            state_file: data_dir.join("state.json"),
            data_dir,
        }
    }

    /// Ensure all data directories exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.tantivy_dir)?;
        Ok(())
    }
}

fn dirs_home() -> PathBuf {
    dirs_home_inner().expect("could not determine home directory")
}

fn dirs_home_inner() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
