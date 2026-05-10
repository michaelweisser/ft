//! Resolve daily-note paths.
//!
//! Three sources, picked by `[daily_notes].source` in ft's config:
//! - `core` — Obsidian's built-in "Daily notes" core plugin
//!   (`<vault>/.obsidian/daily-notes.json`).
//! - `periodic-notes` — the community Periodic Notes plugin
//!   (`<vault>/.obsidian/plugins/periodic-notes/data.json`, `daily` block).
//! - `explicit` — `path` and `format` keys under `[daily_notes]` in ft's
//!   config. Both support moment.js patterns (`journal/YYYY`).
//!
//! The plugin sources translate the plugin's own `format` from moment.js to
//! chrono format. For plugin folders, the value is used verbatim because
//! that's what the plugins themselves do — no surprise diff against what
//! Obsidian writes.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Deserialize;
use thiserror::Error;

use crate::config::{DailyNotes, DailySource};

const DEFAULT_FORMAT: &str = "YYYY-MM-DD";
const CORE_CONFIG: &str = ".obsidian/daily-notes.json";
const PERIODIC_CONFIG: &str = ".obsidian/plugins/periodic-notes/data.json";

#[derive(Debug, Error)]
pub enum DailyError {
    #[error("daily-notes config not found at {}\nhint: enable Obsidian's \"Daily notes\" core plugin, switch [daily_notes].source to \"periodic-notes\" or \"explicit\", or pass --file <PATH>", .path.display())]
    CoreNotFound { path: PathBuf },
    #[error("periodic-notes plugin config not found at {}\nhint: install/enable the Periodic Notes community plugin, switch [daily_notes].source to \"core\" or \"explicit\", or pass --file <PATH>", .path.display())]
    PeriodicNotFound { path: PathBuf },
    #[error("periodic-notes plugin's daily notes are disabled (daily.enabled = false in {}). Enable it in Obsidian, or set [daily_notes].source = \"explicit\".", .path.display())]
    PeriodicDailyDisabled { path: PathBuf },
    #[error("[daily_notes].source = \"explicit\" requires `path` to be set in ft's config")]
    ExplicitMissingPath,
    #[error("could not read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("daily-notes format token `{token}` is not supported")]
    UnsupportedToken { token: String },
}

/// Resolve the absolute path of the daily note for `date`, using the
/// configured source.
pub fn resolve_daily_path(
    vault_root: &Path,
    cfg: &DailyNotes,
    date: NaiveDate,
) -> Result<PathBuf, DailyError> {
    match cfg.source {
        DailySource::Core => resolve_from_core(vault_root, date),
        DailySource::PeriodicNotes => resolve_from_periodic(vault_root, date),
        DailySource::Explicit => resolve_from_explicit(vault_root, cfg, date),
    }
}

// ── core plugin ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct CoreConfig {
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    format: Option<String>,
}

fn resolve_from_core(vault_root: &Path, date: NaiveDate) -> Result<PathBuf, DailyError> {
    let path = vault_root.join(CORE_CONFIG);
    if !path.exists() {
        return Err(DailyError::CoreNotFound { path });
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| DailyError::Read {
        path: path.clone(),
        source: e,
    })?;
    let cfg: CoreConfig = serde_json::from_str(&raw).map_err(|e| DailyError::Parse {
        path: path.clone(),
        source: e,
    })?;
    let folder = cfg.folder.as_deref().unwrap_or("");
    let format = cfg
        .format
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_FORMAT);
    join_daily_path(vault_root, folder, format, date)
}

// ── periodic-notes plugin ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct PeriodicConfig {
    #[serde(default)]
    daily: Option<PeriodicDaily>,
}

#[derive(Debug, Deserialize, Default)]
struct PeriodicDaily {
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    format: Option<String>,
    /// Defaults to `true` when absent — the plugin treats unset as enabled.
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

fn resolve_from_periodic(vault_root: &Path, date: NaiveDate) -> Result<PathBuf, DailyError> {
    let path = vault_root.join(PERIODIC_CONFIG);
    if !path.exists() {
        return Err(DailyError::PeriodicNotFound { path });
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| DailyError::Read {
        path: path.clone(),
        source: e,
    })?;
    let cfg: PeriodicConfig = serde_json::from_str(&raw).map_err(|e| DailyError::Parse {
        path: path.clone(),
        source: e,
    })?;
    let daily = cfg.daily.unwrap_or_default();
    if !daily.enabled {
        return Err(DailyError::PeriodicDailyDisabled { path });
    }
    let folder = daily.folder.as_deref().unwrap_or("");
    let format = daily
        .format
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_FORMAT);
    join_daily_path(vault_root, folder, format, date)
}

// ── explicit ─────────────────────────────────────────────────────────────────

fn resolve_from_explicit(
    vault_root: &Path,
    cfg: &DailyNotes,
    date: NaiveDate,
) -> Result<PathBuf, DailyError> {
    let raw_path = cfg.path.as_deref().ok_or(DailyError::ExplicitMissingPath)?;
    let format = cfg.format.as_deref().unwrap_or(DEFAULT_FORMAT);

    // Both path and format support moment.js patterns in explicit mode.
    let chrono_path_fmt = translate_format(raw_path)?;
    let formatted_folder = date.format(&chrono_path_fmt).to_string();
    let chrono_filename_fmt = translate_format(format)?;
    let filename = date.format(&chrono_filename_fmt).to_string();

    let mut p = vault_root.to_path_buf();
    if !formatted_folder.is_empty() {
        p.push(formatted_folder);
    }
    p.push(format!("{filename}.md"));
    Ok(p)
}

// ── shared helpers ───────────────────────────────────────────────────────────

/// Join `folder` (treated as a literal path) with `format` (translated from
/// moment.js to chrono and resolved against `date`) under `vault_root`.
fn join_daily_path(
    vault_root: &Path,
    folder: &str,
    format: &str,
    date: NaiveDate,
) -> Result<PathBuf, DailyError> {
    let chrono_fmt = translate_format(format)?;
    let filename = date.format(&chrono_fmt).to_string();
    let mut p = vault_root.to_path_buf();
    if !folder.is_empty() {
        p.push(folder);
    }
    p.push(format!("{filename}.md"));
    Ok(p)
}

/// Translate the supported subset of moment.js format tokens to chrono format.
///
/// Supported tokens (greedy, longest-first):
///   `YYYY` → `%Y`        4-digit year
///   `YY`   → `%y`        2-digit year
///   `MMMM` → `%B`        full month name
///   `MMM`  → `%b`        abbreviated month name
///   `MM`   → `%m`        2-digit month
///   `M`    → `%-m`       1- or 2-digit month
///   `DDDD` → `%j`        day of year (zero-padded)
///   `DD`   → `%d`        2-digit day of month
///   `D`    → `%-d`       1- or 2-digit day of month
///   `dddd` → `%A`        full weekday name
///   `ddd`  → `%a`        abbreviated weekday name
///   `HH`   → `%H`        24-hour hour
///   `mm`   → `%M`        minutes
///   `ss`   → `%S`        seconds
///
/// Inside `[...]` brackets, content is passed through verbatim. Any other
/// character that doesn't start a known token is also passed through, matching
/// moment.js's own permissive behavior — so `journal/YYYY` works without
/// needing brackets around `journal`. To embed a literal that does start with
/// a token character (`Y`, `M`, `D`, `H`, `d`, `m`, `s`), wrap it in brackets:
/// `[Daily-]YYYY-MM-DD`.
///
/// Reserved tokens that would conflict with future moment.js support
/// (currently `Q`/`Qo` for quarter) reject with `UnsupportedToken` so we don't
/// silently produce garbage if a user copies a plugin format we don't yet
/// parse.
pub fn translate_format(moment: &str) -> Result<String, DailyError> {
    const TOKENS: &[(&str, &str)] = &[
        ("YYYY", "%Y"),
        ("YY", "%y"),
        ("MMMM", "%B"),
        ("MMM", "%b"),
        ("MM", "%m"),
        ("M", "%-m"),
        ("DDDD", "%j"),
        ("DD", "%d"),
        ("D", "%-d"),
        ("dddd", "%A"),
        ("ddd", "%a"),
        ("HH", "%H"),
        ("mm", "%M"),
        ("ss", "%S"),
    ];
    /// Tokens we recognize as moment.js but don't yet translate. Reject these
    /// loudly so a user who pastes in a plugin format we don't support gets a
    /// clear error rather than silent garbage.
    const RESERVED: &[&str] = &["Qo", "Q"];

    let mut out = String::with_capacity(moment.len());
    let bytes = moment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = moment[i + 1..].find(']') {
                out.push_str(&moment[i + 1..i + 1 + end]);
                i += end + 2;
                continue;
            }
        }

        let rest = &moment[i..];

        if let Some(token) = RESERVED.iter().find(|t| rest.starts_with(*t)) {
            return Err(DailyError::UnsupportedToken {
                token: (*token).to_string(),
            });
        }

        let mut matched = false;
        for (token, repl) in TOKENS {
            if rest.starts_with(token) {
                out.push_str(repl);
                i += token.len();
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }

        let ch = bytes[i] as char;
        if ch == '%' {
            out.push_str("%%");
        } else {
            out.push(ch);
        }
        i += ch.len_utf8();
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;
    use assert_fs::TempDir;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    // ── translate_format ─────────────────────────────────────────────────────

    #[test]
    fn translate_default() {
        assert_eq!(translate_format("YYYY-MM-DD").unwrap(), "%Y-%m-%d");
    }

    #[test]
    fn translate_long_year_and_month() {
        assert_eq!(translate_format("YYYY/MMMM/DD").unwrap(), "%Y/%B/%d");
    }

    #[test]
    fn translate_with_literal_brackets() {
        assert_eq!(
            translate_format("[Daily-]YYYY-MM-DD").unwrap(),
            "Daily-%Y-%m-%d"
        );
    }

    #[test]
    fn translate_unsupported_token_rejected() {
        let err = translate_format("YYYY-Q-MM").unwrap_err();
        assert!(matches!(err, DailyError::UnsupportedToken { .. }));
        if let DailyError::UnsupportedToken { token } = err {
            assert_eq!(token, "Q");
        }
    }

    #[test]
    fn translate_path_pattern() {
        // `journal/YYYY` is a path pattern.
        assert_eq!(translate_format("journal/YYYY").unwrap(), "journal/%Y");
        // Nested year/month subdirs.
        assert_eq!(
            translate_format("journal/YYYY/MM").unwrap(),
            "journal/%Y/%m"
        );
    }

    // ── core mode ────────────────────────────────────────────────────────────

    fn vault_with_core(folder: &str, format: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();
        dir.child(".obsidian").create_dir_all().unwrap();
        let json = match format {
            Some(f) => format!(r#"{{"folder":"{folder}","format":"{f}"}}"#),
            None => format!(r#"{{"folder":"{folder}"}}"#),
        };
        dir.child(CORE_CONFIG).write_str(&json).unwrap();
        dir
    }

    #[test]
    fn core_resolves_with_default_format() {
        let dir = vault_with_core("journal/2024", None);
        let cfg = DailyNotes {
            source: DailySource::Core,
            ..Default::default()
        };
        let p = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap();
        assert_eq!(p, dir.path().join("journal/2024/2026-05-09.md"));
    }

    #[test]
    fn core_missing_errors() {
        let dir = TempDir::new().unwrap();
        dir.child(".obsidian").create_dir_all().unwrap();
        let cfg = DailyNotes::default();
        let err = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap_err();
        assert!(matches!(err, DailyError::CoreNotFound { .. }));
    }

    // ── periodic-notes mode ──────────────────────────────────────────────────

    fn vault_with_periodic(daily_block: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        dir.child(".obsidian/plugins/periodic-notes")
            .create_dir_all()
            .unwrap();
        let json = format!(r#"{{"daily":{daily_block}}}"#);
        dir.child(PERIODIC_CONFIG).write_str(&json).unwrap();
        dir
    }

    #[test]
    fn periodic_resolves_with_empty_format_defaulting() {
        // Real fortytwo shape: format = "" should default to YYYY-MM-DD.
        let dir = vault_with_periodic(r#"{"folder":"journal/2026","format":"","enabled":true}"#);
        let cfg = DailyNotes {
            source: DailySource::PeriodicNotes,
            ..Default::default()
        };
        let p = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap();
        assert_eq!(p, dir.path().join("journal/2026/2026-05-09.md"));
    }

    #[test]
    fn periodic_disabled_errors() {
        let dir = vault_with_periodic(r#"{"folder":"journal","format":"","enabled":false}"#);
        let cfg = DailyNotes {
            source: DailySource::PeriodicNotes,
            ..Default::default()
        };
        let err = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap_err();
        assert!(matches!(err, DailyError::PeriodicDailyDisabled { .. }));
    }

    #[test]
    fn periodic_missing_errors() {
        let dir = TempDir::new().unwrap();
        dir.child(".obsidian").create_dir_all().unwrap();
        let cfg = DailyNotes {
            source: DailySource::PeriodicNotes,
            ..Default::default()
        };
        let err = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap_err();
        assert!(matches!(err, DailyError::PeriodicNotFound { .. }));
    }

    #[test]
    fn periodic_enabled_default_true_when_absent() {
        // When `enabled` key is missing the plugin treats it as enabled; we
        // mirror that.
        let dir = vault_with_periodic(r#"{"folder":"journal","format":""}"#);
        let cfg = DailyNotes {
            source: DailySource::PeriodicNotes,
            ..Default::default()
        };
        let p = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap();
        assert_eq!(p, dir.path().join("journal/2026-05-09.md"));
    }

    // ── explicit mode ────────────────────────────────────────────────────────

    #[test]
    fn explicit_with_path_pattern() {
        let dir = TempDir::new().unwrap();
        let cfg = DailyNotes {
            source: DailySource::Explicit,
            path: Some("journal/YYYY".into()),
            format: None,
        };
        let p = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap();
        assert_eq!(p, dir.path().join("journal/2026/2026-05-09.md"));
    }

    #[test]
    fn explicit_with_year_month_pattern() {
        let dir = TempDir::new().unwrap();
        let cfg = DailyNotes {
            source: DailySource::Explicit,
            path: Some("journal/YYYY/MM".into()),
            format: Some("DD-dddd".into()),
        };
        let p = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap();
        assert_eq!(p, dir.path().join("journal/2026/05/09-Saturday.md"));
    }

    #[test]
    fn explicit_static_path() {
        let dir = TempDir::new().unwrap();
        let cfg = DailyNotes {
            source: DailySource::Explicit,
            path: Some("inbox".into()),
            format: Some("YYYY-MM-DD".into()),
        };
        let p = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap();
        assert_eq!(p, dir.path().join("inbox/2026-05-09.md"));
    }

    #[test]
    fn explicit_requires_path() {
        let dir = TempDir::new().unwrap();
        let cfg = DailyNotes {
            source: DailySource::Explicit,
            path: None,
            format: None,
        };
        let err = resolve_daily_path(dir.path(), &cfg, date(2026, 5, 9)).unwrap_err();
        assert!(matches!(err, DailyError::ExplicitMissingPath));
    }
}
