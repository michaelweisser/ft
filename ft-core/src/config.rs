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
use crate::git::PullStrategy;

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
    /// Per-period configuration for periodic notes (daily/weekly/monthly/
    /// quarterly/yearly). Only configured periods are accessible from the
    /// CLI and TUI; unset periods are surfaced as a "not configured" error
    /// at use time.
    #[serde(default)]
    pub periodic_notes: PeriodicNotes,
    /// Glob patterns (relative to vault root) to exclude from scanning.
    #[serde(default)]
    pub ignored_paths: Vec<String>,
    /// Named task queries (presets). Keys are preset names; values are DSL strings.
    #[serde(default)]
    pub presets: HashMap<String, String>,
    /// Note-creation settings.
    #[serde(default)]
    pub notes: Notes,
    /// Editor handoff strategy and popup geometry. See [`Editor`].
    #[serde(default)]
    pub editor: Editor,
    /// Git-sync settings. See [`Git`].
    #[serde(default)]
    pub git: Git,
    /// Timeblocks settings. See [`Timeblocks`].
    #[serde(default)]
    pub timeblocks: Timeblocks,
}

impl Config {
    /// Heading under which timeblock entries live in each daily note.
    /// Defaults to `"Time Blocks"` when [`Timeblocks::heading`] is unset.
    pub fn timeblocks_heading(&self) -> &str {
        self.timeblocks.heading.as_deref().unwrap_or("Time Blocks")
    }
}

/// `ft timeblocks` configuration. The daily-note path is resolved via the
/// existing [`PeriodicNotes::daily`] block; this struct just controls the
/// section heading the timeblock list is stored under.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Timeblocks {
    /// Heading under which timeblocks live. Defaults to `"Time Blocks"`.
    pub heading: Option<String>,
}

/// `ft git sync` / TUI `g s` configuration. Currently just the pull
/// strategy — conflict handling is identical for both variants and
/// not user-tunable (markers in place, abort before push).
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Git {
    #[serde(default)]
    pub pull_strategy: PullStrategy,
}

/// Settings for `ft notes create` and the TUI create flows.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Notes {
    /// Folder (vault-relative) holding ft-compatible templates. Defaults
    /// to `templates-ft` when unset. See plan 009 for the rationale.
    pub templates_dir: Option<String>,
}

/// Per-period configuration for periodic notes.
///
/// Each field is `Option` so a user can configure only the periods they
/// use; missing entries surface as "period not configured" errors when a
/// caller asks for them.
///
/// Path and filename patterns use chrono `%`-tokens (see [`crate::periodic`]
/// for the supported set, including the `%q`/`%Q` quarter extensions).
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodicNotes {
    pub daily: Option<PeriodicPeriod>,
    pub weekly: Option<PeriodicPeriod>,
    pub monthly: Option<PeriodicPeriod>,
    pub quarterly: Option<PeriodicPeriod>,
    pub yearly: Option<PeriodicPeriod>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodicPeriod {
    /// Folder pattern, vault-relative. Chrono strftime tokens supported,
    /// plus the `%q`/`%Q` quarter extensions from [`crate::periodic`].
    /// Empty string means "vault root".
    pub path: String,
    /// Filename pattern (without `.md`). Same token surface as `path`.
    pub format: String,
    /// Template name resolved under `[notes].templates_dir` (or an
    /// absolute path). When unset, the new note's body is `# <title>\n\n`.
    pub template: Option<String>,
}

/// Editor handoff configuration — how the TUI launches `$EDITOR` when
/// a tab raises `OpenInEditor`.
///
/// The `tmux-*` strategies require ft to be running inside tmux
/// (`$TMUX` env var set); when ft is invoked outside tmux, any
/// `tmux-*` value collapses to [`EditorStrategy::Suspend`] at use time
/// via [`EditorStrategy::resolve`].
///
/// Default: `tmux-popup`. Outside tmux this resolves to `suspend`, so
/// users who don't use tmux see no behavior change versus pre-plan-011.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Editor {
    #[serde(default)]
    pub strategy: EditorStrategy,
    /// Popup width passed to `tmux display-popup -w` when
    /// `strategy = tmux-popup`. Accepts tmux geometry syntax:
    /// percentages (`"90%"`) or cell counts (`"120"`).
    #[serde(default = "default_popup_width")]
    pub popup_width: String,
    /// Popup height — same syntax as [`Self::popup_width`].
    #[serde(default = "default_popup_height")]
    pub popup_height: String,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            strategy: EditorStrategy::default(),
            popup_width: default_popup_width(),
            popup_height: default_popup_height(),
        }
    }
}

fn default_popup_width() -> String {
    "90%".into()
}
fn default_popup_height() -> String {
    "90%".into()
}

/// How to launch `$EDITOR` from the TUI. See [`Editor`] for the
/// `$TMUX`-fallback rule and the geometry knobs that apply only to
/// `TmuxPopup`.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EditorStrategy {
    /// `tmux display-popup -E -- <editor> +N <path>`. The popup's
    /// lifetime is bound to the editor; ESC and other keys forward
    /// through to the editor, the popup closes when the editor exits.
    /// Requires tmux ≥ 3.2.
    #[default]
    TmuxPopup,
    /// `tmux new-window -- <editor> +N <path>`. Editor lands in a new
    /// tmux window; ft stays visible in the original window. ft
    /// blocks via a `tmux wait-for` handshake so the post-edit
    /// refresh runs when the editor closes.
    TmuxWindow,
    /// `tmux split-window -- <editor> +N <path>`. Editor lands in a
    /// split of the current pane. Same `wait-for` handshake as
    /// `TmuxWindow`.
    TmuxSplit,
    /// Current behavior — suspend ft's alt-screen, run the editor
    /// inline, restore the alt-screen on exit. The `tmux-*`
    /// strategies all fall back to this when `$TMUX` is unset.
    Suspend,
}

impl EditorStrategy {
    /// Returns the effective strategy after applying the `$TMUX`
    /// fallback rule. When ft is not running inside tmux, every
    /// `tmux-*` value collapses to `Suspend`. `Suspend` is the
    /// identity.
    ///
    /// Pure modulo the env-var read; tests toggle `$TMUX` to drive
    /// both branches.
    pub fn resolve(self) -> Self {
        let in_tmux = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty());
        match self {
            Self::Suspend => Self::Suspend,
            Self::TmuxPopup | Self::TmuxWindow | Self::TmuxSplit if !in_tmux => Self::Suspend,
            other => other,
        }
    }
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
        assert!(lc.config.periodic_notes.daily.is_none());
        assert!(lc.config.periodic_notes.weekly.is_none());
        assert!(lc.config.periodic_notes.monthly.is_none());
        assert!(lc.config.periodic_notes.quarterly.is_none());
        assert!(lc.config.periodic_notes.yearly.is_none());
        assert!(!lc.sources[0].present);
        assert!(!lc.sources[1].present);
    }

    #[test]
    fn periodic_notes_daily_only() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[periodic_notes.daily]
path = "journal/%Y"
format = "%Y-%m-%d"
template = "daily"
"#,
        )
        .unwrap();

        let lc = load(user.path(), &tmp.path().join("no-vault.toml")).unwrap();
        let d = lc.config.periodic_notes.daily.as_ref().unwrap();
        assert_eq!(d.path, "journal/%Y");
        assert_eq!(d.format, "%Y-%m-%d");
        assert_eq!(d.template.as_deref(), Some("daily"));
        assert!(lc.config.periodic_notes.weekly.is_none());
    }

    #[test]
    fn periodic_notes_all_five_periods() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[periodic_notes.daily]
path = "journal/%Y"
format = "%Y-%m-%d"

[periodic_notes.weekly]
path = "journal/%Y"
format = "%G-W%V"

[periodic_notes.monthly]
path = "journal/%Y"
format = "%Y-%m"

[periodic_notes.quarterly]
path = "journal/%Y"
format = "%Y-Q%q"

[periodic_notes.yearly]
path = "journal"
format = "%Y"
"#,
            )
            .unwrap();
        let lc = load(&tmp.path().join("no-user.toml"), vault.path()).unwrap();
        assert!(lc.config.periodic_notes.daily.is_some());
        assert!(lc.config.periodic_notes.weekly.is_some());
        assert!(lc.config.periodic_notes.monthly.is_some());
        assert!(lc.config.periodic_notes.quarterly.is_some());
        assert!(lc.config.periodic_notes.yearly.is_some());
        assert_eq!(
            lc.config.periodic_notes.quarterly.as_ref().unwrap().format,
            "%Y-Q%q"
        );
    }

    #[test]
    fn vault_config_wins_over_user_for_periodic_block() {
        let tmp = TempDir::new().unwrap();
        let user = tmp.child("user.toml");
        user.write_str(
            r#"
[periodic_notes.daily]
path = "from-user"
format = "%Y-%m-%d"
"#,
        )
        .unwrap();

        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[periodic_notes.daily]
path = "from-vault"
format = "%Y-%m-%d"
"#,
            )
            .unwrap();

        let lc = load(user.path(), vault.path()).unwrap();
        assert_eq!(
            lc.config.periodic_notes.daily.as_ref().unwrap().path,
            "from-vault"
        );
    }

    #[test]
    fn old_daily_notes_block_rejected() {
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
        let r = load(user.path(), &tmp.path().join("no-vault.toml"));
        assert!(
            r.is_err(),
            "old [daily_notes] block should be rejected by deny_unknown_fields"
        );
    }

    #[test]
    fn periodic_notes_typo_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[periodic_notes.daily]
path = "journal"
format = "%Y-%m-%d"
typo_field = "oops"
"#,
            )
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err());
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
    fn notes_templates_dir_default_is_none() {
        let tmp = TempDir::new().unwrap();
        let lc = load(
            &tmp.path().join("no-user.toml"),
            &tmp.path().join("no-vault.toml"),
        )
        .unwrap();
        assert!(lc.config.notes.templates_dir.is_none());
    }

    #[test]
    fn notes_templates_dir_override() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[notes]
templates_dir = "_templates"
"#,
            )
            .unwrap();
        let lc = load(&tmp.path().join("no-user.toml"), vault.path()).unwrap();
        assert_eq!(lc.config.notes.templates_dir.as_deref(), Some("_templates"));
    }

    #[test]
    fn notes_unknown_key_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[notes]
typo_field = "oops"
"#,
            )
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err());
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

    // ── [editor] block (plan 011 session 1) ──────────────────────────────

    /// Shared mutex so the `$TMUX`-toggling tests don't race each other
    /// (or other tests that read env vars). Local to the editor tests
    /// because `vault.rs` has its own copy for its own env-var tests.
    static EDITOR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn editor_defaults_when_block_absent() {
        let tmp = TempDir::new().unwrap();
        let lc = load(
            &tmp.path().join("no-user.toml"),
            &tmp.path().join("no-vault.toml"),
        )
        .unwrap();
        assert_eq!(lc.config.editor.strategy, EditorStrategy::TmuxPopup);
        assert_eq!(lc.config.editor.popup_width, "90%");
        assert_eq!(lc.config.editor.popup_height, "90%");
    }

    #[test]
    fn editor_strategy_kebab_case_variants_parse() {
        for (s, expected) in [
            ("tmux-popup", EditorStrategy::TmuxPopup),
            ("tmux-window", EditorStrategy::TmuxWindow),
            ("tmux-split", EditorStrategy::TmuxSplit),
            ("suspend", EditorStrategy::Suspend),
        ] {
            let tmp = TempDir::new().unwrap();
            let vault = tmp.child("vault.toml");
            vault
                .write_str(&format!("[editor]\nstrategy = \"{s}\"\n"))
                .unwrap();
            let lc = load(&tmp.path().join("no-user.toml"), vault.path()).unwrap();
            assert_eq!(
                lc.config.editor.strategy, expected,
                "strategy={s:?} should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn editor_unknown_strategy_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str("[editor]\nstrategy = \"vsplit\"\n")
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err(), "unknown strategy must be rejected");
    }

    #[test]
    fn editor_unknown_field_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str("[editor]\nstrategy = \"suspend\"\ntypo_field = 1\n")
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err(), "deny_unknown_fields should reject typos");
    }

    #[test]
    fn editor_popup_geometry_overrides_apply() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[editor]
strategy = "tmux-popup"
popup_width = "80"
popup_height = "50%"
"#,
            )
            .unwrap();
        let lc = load(&tmp.path().join("no-user.toml"), vault.path()).unwrap();
        assert_eq!(lc.config.editor.popup_width, "80");
        assert_eq!(lc.config.editor.popup_height, "50%");
    }

    #[test]
    fn editor_strategy_resolve_passes_through_when_in_tmux() {
        let _guard = EDITOR_ENV_LOCK.lock().unwrap();
        std::env::set_var("TMUX", "/tmp/tmux-1000/default,1234,0");
        assert_eq!(
            EditorStrategy::TmuxPopup.resolve(),
            EditorStrategy::TmuxPopup
        );
        assert_eq!(
            EditorStrategy::TmuxWindow.resolve(),
            EditorStrategy::TmuxWindow
        );
        assert_eq!(
            EditorStrategy::TmuxSplit.resolve(),
            EditorStrategy::TmuxSplit
        );
        assert_eq!(EditorStrategy::Suspend.resolve(), EditorStrategy::Suspend);
        std::env::remove_var("TMUX");
    }

    #[test]
    fn editor_strategy_resolve_falls_back_to_suspend_outside_tmux() {
        let _guard = EDITOR_ENV_LOCK.lock().unwrap();
        std::env::remove_var("TMUX");
        assert_eq!(EditorStrategy::TmuxPopup.resolve(), EditorStrategy::Suspend);
        assert_eq!(
            EditorStrategy::TmuxWindow.resolve(),
            EditorStrategy::Suspend
        );
        assert_eq!(EditorStrategy::TmuxSplit.resolve(), EditorStrategy::Suspend);
        assert_eq!(EditorStrategy::Suspend.resolve(), EditorStrategy::Suspend);
    }

    // ── [git] block (plan 012 session 1) ─────────────────────────────────

    #[test]
    fn git_defaults_when_block_absent() {
        let tmp = TempDir::new().unwrap();
        let lc = load(
            &tmp.path().join("no-user.toml"),
            &tmp.path().join("no-vault.toml"),
        )
        .unwrap();
        assert_eq!(lc.config.git.pull_strategy, PullStrategy::Merge);
    }

    #[test]
    fn git_pull_strategy_kebab_case_variants_parse() {
        for (s, expected) in [
            ("merge", PullStrategy::Merge),
            ("rebase", PullStrategy::Rebase),
        ] {
            let tmp = TempDir::new().unwrap();
            let vault = tmp.child("vault.toml");
            vault
                .write_str(&format!("[git]\npull_strategy = \"{s}\"\n"))
                .unwrap();
            let lc = load(&tmp.path().join("no-user.toml"), vault.path()).unwrap();
            assert_eq!(
                lc.config.git.pull_strategy, expected,
                "pull_strategy={s:?} should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn git_unknown_strategy_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str("[git]\npull_strategy = \"squash\"\n")
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err(), "unknown pull_strategy must be rejected");
    }

    #[test]
    fn git_unknown_field_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str("[git]\npull_strategy = \"merge\"\ntypo_field = 1\n")
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err(), "deny_unknown_fields should reject typos");
    }

    // ── [editor] block (plan 011 session 1) — continued ──────────────────

    // ── [timeblocks] block (plan 015 session 1) ──────────────────────────

    #[test]
    fn timeblocks_default_heading_is_time_blocks() {
        let tmp = TempDir::new().unwrap();
        let lc = load(
            &tmp.path().join("no-user.toml"),
            &tmp.path().join("no-vault.toml"),
        )
        .unwrap();
        assert!(lc.config.timeblocks.heading.is_none());
        assert_eq!(lc.config.timeblocks_heading(), "Time Blocks");
    }

    #[test]
    fn timeblocks_heading_override_applies() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[timeblocks]
heading = "Day Planner"
"#,
            )
            .unwrap();
        let lc = load(&tmp.path().join("no-user.toml"), vault.path()).unwrap();
        assert_eq!(lc.config.timeblocks_heading(), "Day Planner");
    }

    #[test]
    fn timeblocks_unknown_field_rejected() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.child("vault.toml");
        vault
            .write_str(
                r#"
[timeblocks]
heading = "Time Blocks"
typo_field = "oops"
"#,
            )
            .unwrap();
        let r = load(&tmp.path().join("no-user.toml"), vault.path());
        assert!(r.is_err(), "deny_unknown_fields should reject typos");
    }

    #[test]
    fn editor_strategy_resolve_treats_empty_tmux_as_unset() {
        // tmux unsets TMUX inside `tmux detach` and a few other paths;
        // an empty value should be treated like "not in tmux".
        let _guard = EDITOR_ENV_LOCK.lock().unwrap();
        std::env::set_var("TMUX", "");
        assert_eq!(EditorStrategy::TmuxPopup.resolve(), EditorStrategy::Suspend);
        std::env::remove_var("TMUX");
    }
}
