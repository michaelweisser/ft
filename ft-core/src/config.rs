use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use figment::{
    providers::{Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Top-level `ft` configuration.
///
/// Unknown keys are rejected with a clear error message so typos are caught
/// immediately rather than silently ignored.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Default vault path (valid in user config only).
    /// Used as last-resort in vault discovery when no other signal is present.
    pub default_vault: Option<String>,
    /// Default file for new tasks, relative to vault root.
    pub default_task_location: Option<String>,
    /// How to resolve the daily-note path. Pick exactly one source.
    #[serde(default)]
    pub daily_notes: DailyNotes,
    /// Glob patterns (relative to vault root) to exclude from scanning.
    #[serde(default)]
    pub ignored_paths: Vec<String>,
    /// Named task queries (presets). Keys are preset names; values are DSL strings.
    #[serde(default)]
    pub presets: HashMap<String, String>,
}

/// Where the daily-note path comes from.
///
/// - `Core` reads `<vault>/.obsidian/daily-notes.json` (Obsidian's built-in
///   "Daily notes" core plugin).
/// - `PeriodicNotes` reads `<vault>/.obsidian/plugins/periodic-notes/data.json`.
///   Only the `daily` block is consulted.
/// - `Explicit` ignores both plugins and uses [`DailyNotes::path`] /
///   [`DailyNotes::format`] verbatim. Both fields support moment.js patterns,
///   so `path = "journal/YYYY"` resolves to `journal/2026/…` automatically.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DailySource {
    #[default]
    Core,
    PeriodicNotes,
    Explicit,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DailyNotes {
    #[serde(default)]
    pub source: DailySource,
    /// Folder pattern, used only when `source = "explicit"`. Supports moment.js
    /// tokens (`YYYY`, `MM`, `[literal]`, etc.) and is resolved against the
    /// target date.
    pub path: Option<String>,
    /// Filename pattern (without `.md`), used only when `source = "explicit"`.
    /// Defaults to `YYYY-MM-DD` when unset.
    pub format: Option<String>,
}

#[derive(Debug)]
pub struct ConfigSource {
    /// Human-readable label: "user" or "vault".
    pub label: String,
    pub path: PathBuf,
    /// Whether the file exists on disk.
    pub present: bool,
}

#[derive(Debug)]
pub struct LayeredConfig {
    pub config: Config,
    /// Sources in order of increasing precedence (last = highest priority).
    pub sources: Vec<ConfigSource>,
}

/// Load configuration by merging user-level and vault-level TOML files.
///
/// Vault config wins over user config. Missing files are silently skipped.
pub fn load(user_config: &Path, vault_config: &Path) -> Result<LayeredConfig> {
    let config = Figment::new()
        .merge(Serialized::defaults(Config::default()))
        .merge(Toml::file(user_config))
        .merge(Toml::file(vault_config))
        .extract::<Config>()
        .map_err(|e| Error::Config {
            path: if vault_config.exists() {
                vault_config.display().to_string()
            } else {
                user_config.display().to_string()
            },
            source: Box::new(e),
        })?;

    Ok(LayeredConfig {
        config,
        sources: vec![
            ConfigSource {
                label: "user".into(),
                path: user_config.to_path_buf(),
                present: user_config.exists(),
            },
            ConfigSource {
                label: "vault".into(),
                path: vault_config.to_path_buf(),
                present: vault_config.exists(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;
    use assert_fs::TempDir;

    #[test]
    fn defaults_when_no_files() {
        let tmp = TempDir::new().unwrap();
        let lc = load(
            &tmp.path().join("nonexistent-user.toml"),
            &tmp.path().join("nonexistent-vault.toml"),
        )
        .unwrap();
        assert!(lc.config.default_task_location.is_none());
        assert!(lc.config.ignored_paths.is_empty());
        assert!(!lc.sources[0].present);
        assert!(!lc.sources[1].present);
    }

    #[test]
    fn user_config_only() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[daily_notes]
source = "explicit"
path = "Journal"
format = "YYYY-MM-DD"
"#,
        )
        .unwrap();

        let lc = load(user.path(), &tmp.path().join("no-vault.toml")).unwrap();
        assert_eq!(lc.config.daily_notes.source, DailySource::Explicit);
        assert_eq!(lc.config.daily_notes.path.as_deref(), Some("Journal"));
        assert_eq!(lc.config.daily_notes.format.as_deref(), Some("YYYY-MM-DD"));
        assert!(lc.sources[0].present);
        assert!(!lc.sources[1].present);
    }

    #[test]
    fn daily_source_defaults_to_core() {
        let tmp = TempDir::new().unwrap();
        let lc = load(
            &tmp.path().join("no-user.toml"),
            &tmp.path().join("no-vault.toml"),
        )
        .unwrap();
        assert_eq!(lc.config.daily_notes.source, DailySource::Core);
        assert!(lc.config.daily_notes.path.is_none());
    }

    #[test]
    fn daily_source_periodic_notes() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[daily_notes]
source = "periodic-notes"
"#,
        )
        .unwrap();
        let lc = load(user.path(), &tmp.path().join("no-vault.toml")).unwrap();
        assert_eq!(lc.config.daily_notes.source, DailySource::PeriodicNotes);
    }

    #[test]
    fn vault_config_wins_over_user() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[daily_notes]
source = "explicit"
path = "from-user"
"#,
        )
        .unwrap();

        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[daily_notes]
source = "explicit"
path = "from-vault"
"#,
            )
            .unwrap();

        let lc = load(user.path(), vault.path()).unwrap();
        assert_eq!(lc.config.daily_notes.path.as_deref(), Some("from-vault"));
    }

    #[test]
    fn vault_config_merges_non_overlapping_keys() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[daily_notes]
source = "explicit"
path = "Journal"
"#,
        )
        .unwrap();

        let vault = tmp.child("vault.toml");
        vault
            .write_str(r#"default_task_location = "Tasks.md""#)
            .unwrap();

        let lc = load(user.path(), vault.path()).unwrap();
        assert_eq!(lc.config.daily_notes.path.as_deref(), Some("Journal"));
        assert_eq!(lc.config.default_task_location.as_deref(), Some("Tasks.md"));
    }

    #[test]
    fn unknown_key_in_config_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(r#"typo_key = "oops""#).unwrap();

        let result = load(user.path(), &tmp.path().join("no-vault.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn presets_loaded_correctly() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[presets]
work = "tag is #work and not done"
"#,
        )
        .unwrap();

        let lc = load(user.path(), &tmp.path().join("no-vault.toml")).unwrap();
        assert_eq!(
            lc.config.presets.get("work").map(|s| s.as_str()),
            Some("tag is #work and not done")
        );
    }
}
