//! OS-native scheduled reindex installer.
//!
//! Writes a systemd user service+timer on Linux, a launchd plist on macOS, or
//! prints a suggested cron line on anything else. All installers are
//! idempotent: re-running `install` replaces the existing unit.

use std::path::{Path, PathBuf};

/// Target platform for the installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Systemd,
    Launchd,
    Cron,
}

pub fn detect_platform() -> Platform {
    if cfg!(target_os = "linux") {
        if which("systemctl").is_some() {
            return Platform::Systemd;
        }
        return Platform::Cron;
    }
    if cfg!(target_os = "macos") {
        return Platform::Launchd;
    }
    Platform::Cron
}

/// Parse a human interval like "hourly", "30m", "2h", "daily" into a (systemd
/// OnCalendar, launchd StartInterval seconds, cron expression) triple.
pub struct Interval {
    pub systemd_on_calendar: String,
    pub launchd_seconds: u64,
    pub cron_expression: String,
    pub label: String,
}

pub fn parse_interval(input: &str) -> Result<Interval, String> {
    match input {
        "hourly" => Ok(Interval {
            systemd_on_calendar: "hourly".into(),
            launchd_seconds: 3600,
            cron_expression: "0 * * * *".into(),
            label: "hourly".into(),
        }),
        "daily" => Ok(Interval {
            systemd_on_calendar: "daily".into(),
            launchd_seconds: 86400,
            cron_expression: "0 4 * * *".into(),
            label: "daily".into(),
        }),
        s => {
            // Parse "30m" | "2h" | "90s"
            let (num_s, unit) = s.split_at(
                s.find(|c: char| !c.is_ascii_digit())
                    .ok_or_else(|| format!("bad interval: {s}"))?,
            );
            let n: u64 = num_s.parse().map_err(|_| format!("bad interval: {s}"))?;
            let seconds = match unit {
                "s" => n,
                "m" => n * 60,
                "h" => n * 3600,
                "d" => n * 86400,
                _ => return Err(format!("unknown interval unit: {unit}")),
            };
            let (systemd_oc, cron_expr) = match unit {
                "s" => return Err("sub-minute intervals not supported".into()),
                "m" => (
                    format!("*:0/{}", n.clamp(1, 59)),
                    format!("*/{} * * * *", n.clamp(1, 59)),
                ),
                "h" => (
                    format!("*-*-* 0/{}:00:00", n.clamp(1, 23)),
                    format!("0 */{} * * *", n.clamp(1, 23)),
                ),
                "d" => ("daily".into(), "0 4 * * *".into()),
                _ => return Err(format!("unknown interval unit: {unit}")),
            };
            Ok(Interval {
                systemd_on_calendar: systemd_oc,
                launchd_seconds: seconds,
                cron_expression: cron_expr,
                label: s.to_string(),
            })
        }
    }
}

pub fn install(interval: &Interval) -> Result<String, String> {
    let exe = current_exe_abs()?;
    match detect_platform() {
        Platform::Systemd => install_systemd(&exe, interval),
        Platform::Launchd => install_launchd(&exe, interval),
        Platform::Cron => Ok(cron_instructions(&exe, interval)),
    }
}

pub fn uninstall() -> Result<String, String> {
    match detect_platform() {
        Platform::Systemd => uninstall_systemd(),
        Platform::Launchd => uninstall_launchd(),
        Platform::Cron => Ok(
            "No systemd/launchd service installed. If you added a cron entry manually, remove it with `crontab -e`.".into(),
        ),
    }
}

pub fn status() -> Result<String, String> {
    match detect_platform() {
        Platform::Systemd => status_systemd(),
        Platform::Launchd => status_launchd(),
        Platform::Cron => Ok("No native scheduler detected; check `crontab -l` if you added one.".into()),
    }
}

// ── systemd ────────────────────────────────────────────────────────────

fn systemd_user_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME unset")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

fn install_systemd(exe: &Path, interval: &Interval) -> Result<String, String> {
    let dir = systemd_user_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    let service = format!(
        "[Unit]\n\
         Description=dex (Claude Code conversation indexer) — scheduled reindex\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exe} index\n\
         Nice=10\n\
         ",
        exe = exe.display()
    );
    let timer = format!(
        "[Unit]\n\
         Description=dex scheduled reindex ({label})\n\
         \n\
         [Timer]\n\
         OnCalendar={oc}\n\
         Persistent=true\n\
         RandomizedDelaySec=60\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n\
         ",
        label = interval.label,
        oc = interval.systemd_on_calendar
    );

    let svc_path = dir.join("dex-index.service");
    let tim_path = dir.join("dex-index.timer");
    std::fs::write(&svc_path, service).map_err(|e| format!("write service: {e}"))?;
    std::fs::write(&tim_path, timer).map_err(|e| format!("write timer: {e}"))?;

    run("systemctl", &["--user", "daemon-reload"])?;
    run(
        "systemctl",
        &["--user", "enable", "--now", "dex-index.timer"],
    )?;

    Ok(format!(
        "Installed systemd user units:\n  {}\n  {}\nEnabled dex-index.timer ({}).",
        svc_path.display(),
        tim_path.display(),
        interval.label
    ))
}

fn uninstall_systemd() -> Result<String, String> {
    let dir = systemd_user_dir()?;
    let svc = dir.join("dex-index.service");
    let tim = dir.join("dex-index.timer");

    let _ = run("systemctl", &["--user", "disable", "--now", "dex-index.timer"]);
    let mut removed = Vec::new();
    if tim.exists() {
        std::fs::remove_file(&tim).map_err(|e| format!("remove timer: {e}"))?;
        removed.push(tim.display().to_string());
    }
    if svc.exists() {
        std::fs::remove_file(&svc).map_err(|e| format!("remove service: {e}"))?;
        removed.push(svc.display().to_string());
    }
    let _ = run("systemctl", &["--user", "daemon-reload"]);

    if removed.is_empty() {
        Ok("Nothing to uninstall.".into())
    } else {
        Ok(format!("Removed:\n  {}", removed.join("\n  ")))
    }
}

fn status_systemd() -> Result<String, String> {
    let out = std::process::Command::new("systemctl")
        .args([
            "--user",
            "list-timers",
            "dex-index.timer",
            "--no-pager",
            "--all",
        ])
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() || stdout.contains("0 timers listed") {
        Ok("dex-index.timer not installed. Run `dex service install`.".into())
    } else {
        Ok(stdout)
    }
}

// ── launchd ────────────────────────────────────────────────────────────

fn launchd_plist_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME unset")?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents/build.wigwam.dex.plist"))
}

fn install_launchd(exe: &Path, interval: &Interval) -> Result<String, String> {
    let path = launchd_plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>build.wigwam.dex</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>index</string>
  </array>
  <key>StartInterval</key><integer>{seconds}</integer>
  <key>RunAtLoad</key><false/>
  <key>Nice</key><integer>10</integer>
</dict>
</plist>
"#,
        exe = exe.display(),
        seconds = interval.launchd_seconds
    );
    std::fs::write(&path, plist).map_err(|e| format!("write plist: {e}"))?;

    let _ = run("launchctl", &["unload", &path.display().to_string()]);
    run("launchctl", &["load", &path.display().to_string()])?;

    Ok(format!(
        "Installed launchd agent: {} (every {}s)",
        path.display(),
        interval.launchd_seconds
    ))
}

fn uninstall_launchd() -> Result<String, String> {
    let path = launchd_plist_path()?;
    if !path.exists() {
        return Ok("Nothing to uninstall.".into());
    }
    let _ = run("launchctl", &["unload", &path.display().to_string()]);
    std::fs::remove_file(&path).map_err(|e| format!("remove: {e}"))?;
    Ok(format!("Removed {}", path.display()))
}

fn status_launchd() -> Result<String, String> {
    let out = std::process::Command::new("launchctl")
        .args(["list", "build.wigwam.dex"])
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Ok("build.wigwam.dex not loaded. Run `dex service install`.".into())
    }
}

// ── cron fallback ──────────────────────────────────────────────────────

fn cron_instructions(exe: &Path, interval: &Interval) -> String {
    format!(
        "No systemd/launchd detected. Add this line to your crontab (run `crontab -e`):\n\n  {cron} {exe} index >> ~/.local/share/dex/cron.log 2>&1\n",
        cron = interval.cron_expression,
        exe = exe.display()
    )
}

// ── helpers ────────────────────────────────────────────────────────────

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| format!("run {cmd}: {e}"))?;
    if !status.success() {
        return Err(format!("{cmd} {args:?} exited with {status}"));
    }
    Ok(())
}

fn current_exe_abs() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    exe.canonicalize()
        .map_err(|e| format!("canonicalize exe: {e}"))
}

fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
}
