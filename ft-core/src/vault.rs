use std::path::{Path, PathBuf};

use ignore::{overrides::OverrideBuilder, WalkBuilder};
use rayon::prelude::*;
use tracing::debug;

use crate::{
    config::{self, LayeredConfig},
    error::{Error, Result, ScanError},
    task::{
        emoji::EmojiFormat,
        format::{ParseContext, TaskFormat},
        hierarchy::resolve_hierarchy,
        Task,
    },
};

/// Folders excluded from scanning by default. Combined with `.gitignore` and
/// the vault's `ignored_paths` config.
pub const DEFAULT_IGNORED: &[&str] = &[".obsidian", ".git", "attachments"];

#[derive(Debug)]
pub struct Vault {
    pub path: PathBuf,
    pub config: LayeredConfig,
}

/// Result of [`Vault::scan`]. Tasks across the vault, plus per-file errors
/// collected non-fatally.
#[derive(Debug, Default)]
pub struct Scan {
    pub tasks: Vec<Task>,
    pub errors: Vec<ScanError>,
}

impl Vault {
    /// Discover the vault root and load its layered configuration.
    ///
    /// Discovery precedence:
    /// 1. `vault_flag` — from `--vault` CLI flag
    /// 2. `FT_VAULT` environment variable
    /// 3. Walk up from the current working directory looking for `.obsidian/`
    /// 4. `default_vault` key in `~/.config/ft/config.toml`
    ///
    /// If none of the above succeeds, returns [`Error::VaultNotFound`] with
    /// every location that was attempted.
    pub fn discover(vault_flag: Option<PathBuf>) -> Result<Self> {
        let vault_path = find_vault(vault_flag)?;
        debug!(vault = %vault_path.display(), "vault resolved");

        let user_config_path = user_config_dir().join("ft").join("config.toml");
        let vault_config_path = vault_path.join(".ft").join("config.toml");

        let config = config::load(&user_config_path, &vault_config_path)?;

        Ok(Vault {
            path: vault_path,
            config,
        })
    }

    /// Walk the vault, parse every markdown file in parallel, and return all
    /// tasks plus per-file errors. Respects `.gitignore`, default exclusions
    /// (`.obsidian/`, `.git/`, `attachments/`), and the `ignored_paths` config.
    pub fn scan(&self) -> Scan {
        let files = self.markdown_files();
        debug!(file_count = files.len(), "starting parallel parse");

        let results: Vec<(Vec<Task>, Option<ScanError>)> = files
            .par_iter()
            .map(|path| parse_file(&self.path, path))
            .collect();

        let mut scan = Scan::default();
        for (tasks, err) in results {
            scan.tasks.extend(tasks);
            if let Some(e) = err {
                scan.errors.push(e);
            }
        }
        scan
    }

    /// Walk the vault and pair each markdown file with its `mtime`.
    ///
    /// Same exclusion rules as [`Self::markdown_files`]. Files whose
    /// metadata can't be read are kept in the result with mtime set to
    /// `SystemTime::UNIX_EPOCH` so they still appear (last) in any
    /// recency ranking rather than being silently dropped.
    pub(crate) fn markdown_files_with_mtime(&self) -> Vec<(PathBuf, std::time::SystemTime)> {
        self.markdown_files()
            .into_iter()
            .map(|p| {
                let mtime = std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                (p, mtime)
            })
            .collect()
    }

    pub(crate) fn markdown_files(&self) -> Vec<PathBuf> {
        let mut overrides = OverrideBuilder::new(&self.path);
        for default in DEFAULT_IGNORED {
            // `!pattern` excludes; trailing `/` keeps it a directory match.
            let _ = overrides.add(&format!("!{default}/**"));
        }
        for extra in &self.config.config.ignored_paths {
            let pattern = if extra.ends_with('/') {
                format!("!{extra}**")
            } else {
                format!("!{extra}")
            };
            let _ = overrides.add(&pattern);
        }
        let overrides = overrides.build().expect("override patterns are valid");

        let walker = WalkBuilder::new(&self.path)
            .hidden(true)
            .ignore(true)
            .git_ignore(true)
            .git_exclude(true)
            .parents(false)
            .overrides(overrides)
            .build();

        let mut files = Vec::new();
        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().is_some_and(|e| e == "md") {
                files.push(path.to_path_buf());
            }
        }
        files
    }

    /// Resolve the target file for a new task: an explicit override if
    /// supplied (joined against the vault root when relative), otherwise
    /// today's daily note. Returns an absolute path.
    ///
    /// Shared by the CLI (`ft tasks create --file`) and the TUI's
    /// quickline `in:PATH` token so both surfaces agree on what "target
    /// file" means for a given vault + day.
    pub fn resolve_target(
        &self,
        today: chrono::NaiveDate,
        file_override: Option<&Path>,
    ) -> std::result::Result<PathBuf, crate::daily::DailyError> {
        if let Some(file) = file_override {
            let p = if file.is_absolute() {
                file.to_path_buf()
            } else {
                self.path.join(file)
            };
            return Ok(p);
        }
        crate::daily::resolve_daily_path(&self.path, &self.config.config.daily_notes, today)
    }
}

fn parse_file(vault_root: &Path, path: &Path) -> (Vec<Task>, Option<ScanError>) {
    let rel = path.strip_prefix(vault_root).unwrap_or(path).to_path_buf();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return (
                Vec::new(),
                Some(ScanError {
                    path: rel,
                    message: format!("read failed: {e}"),
                }),
            );
        }
    };

    let mut tasks = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        let ctx = ParseContext {
            source_file: rel.clone(),
            source_line: lineno + 1,
        };
        if let Some(task) = EmojiFormat.parse_line(line, ctx) {
            tasks.push(task);
        }
    }
    resolve_hierarchy(&mut tasks);
    (tasks, None)
}

fn find_vault(vault_flag: Option<PathBuf>) -> Result<PathBuf> {
    let mut tried: Vec<String> = Vec::new();
    // 1. --vault flag
    if let Some(flag_path) = vault_flag {
        let canonical = flag_path
            .canonicalize()
            .unwrap_or_else(|_| flag_path.clone());
        if canonical.join(".obsidian").exists() {
            debug!("vault from --vault flag: {}", canonical.display());
            return Ok(canonical);
        }
        tried.push(format!(
            "  --vault {}: no .obsidian/ found",
            flag_path.display()
        ));
    } else {
        tried.push("  --vault: not provided".into());
    }

    // 2. FT_VAULT env var
    match std::env::var("FT_VAULT") {
        Ok(val) if !val.is_empty() => {
            let p = PathBuf::from(&val);
            let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
            if canonical.join(".obsidian").exists() {
                debug!("vault from $FT_VAULT: {}", canonical.display());
                return Ok(canonical);
            }
            tried.push(format!("  $FT_VAULT={}: no .obsidian/ found", val));
        }
        _ => {
            tried.push("  $FT_VAULT: not set".into());
        }
    }

    // 3. Walk up from CWD
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(found) = walk_up(&cwd) {
        debug!("vault from CWD walk: {}", found.display());
        return Ok(found);
    }
    tried.push(format!(
        "  CWD walk from {}: no ancestor contains .obsidian/",
        cwd.display()
    ));

    // 4. default_vault in user config
    let user_config_path = user_config_dir().join("ft").join("config.toml");
    if let Some(default_vault) = read_default_vault(&user_config_path) {
        let p = PathBuf::from(&default_vault);
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        if canonical.join(".obsidian").exists() {
            debug!("vault from config default_vault: {}", canonical.display());
            return Ok(canonical);
        }
        tried.push(format!(
            "  {}: default_vault={}: no .obsidian/ found",
            user_config_path.display(),
            default_vault
        ));
    } else {
        tried.push(format!(
            "  {}: default_vault not set",
            user_config_path.display()
        ));
    }

    Err(Error::VaultNotFound { tried })
}

fn walk_up(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".obsidian").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

fn read_default_vault(config_path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(config_path).ok()?;
    let table: toml::Table = raw.parse().ok()?;
    table
        .get("default_vault")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Returns `~/.config` regardless of platform.
/// On macOS, `dirs::config_dir()` returns `~/Library/Application Support`, but
/// we follow the XDG convention (`~/.config`) for portability and simplicity.
fn user_config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;
    use assert_fs::TempDir;

    fn make_obsidian_dir(dir: &TempDir) {
        dir.child(".obsidian").create_dir_all().unwrap();
    }

    // ── flag ─────────────────────────────────────────────────────────────────

    #[test]
    fn flag_pointing_at_valid_vault() {
        let dir = TempDir::new().unwrap();
        make_obsidian_dir(&dir);
        let vault = Vault::discover(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(
            vault.path.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn flag_pointing_at_non_vault_errors() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("FT_VAULT");
        let dir = TempDir::new().unwrap();
        // No .obsidian/ here
        let result = Vault::discover(Some(dir.path().to_path_buf()));
        assert!(matches!(result, Err(Error::VaultNotFound { .. })));
    }

    #[test]
    fn error_message_lists_tried_locations() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("FT_VAULT");
        let dir = TempDir::new().unwrap();
        let err = Vault::discover(Some(dir.path().to_path_buf())).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--vault"),
            "error should mention --vault; got: {msg}"
        );
    }

    // ── walk_up ───────────────────────────────────────────────────────────────

    #[test]
    fn walk_up_finds_obsidian_in_parent() {
        let vault_dir = TempDir::new().unwrap();
        make_obsidian_dir(&vault_dir);
        let sub = vault_dir.child("notes/2026");
        sub.create_dir_all().unwrap();

        let found = walk_up(sub.path()).unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            vault_dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn walk_up_returns_none_when_no_obsidian() {
        let dir = TempDir::new().unwrap();
        assert!(walk_up(dir.path()).is_none());
    }

    #[test]
    fn walk_up_finds_self() {
        let dir = TempDir::new().unwrap();
        make_obsidian_dir(&dir);
        let found = walk_up(dir.path()).unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    // ── find_vault (env) ─────────────────────────────────────────────────────
    // These tests use a global shared resource (the environment) and must not
    // run concurrently.  We use a process-level mutex to serialize them.

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── scan ──────────────────────────────────────────────────────────────────

    fn make_vault_with(files: &[(&str, &str)]) -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        make_obsidian_dir(&dir);
        for (rel, content) in files {
            let f = dir.child(rel);
            f.touch().unwrap();
            f.write_str(content).unwrap();
        }
        let vault = Vault::discover(Some(dir.path().to_path_buf())).unwrap();
        (dir, vault)
    }

    #[test]
    fn scan_collects_tasks_from_multiple_files() {
        let (_dir, vault) = make_vault_with(&[
            ("a.md", "- [ ] task in A\n- [x] done in A ✅ 2026-05-01\n"),
            ("b.md", "Some prose\n- [ ] task in B\n"),
        ]);
        let scan = vault.scan();
        assert_eq!(scan.tasks.len(), 3, "expected 3 tasks total");
        assert!(scan.errors.is_empty());
    }

    #[test]
    fn scan_skips_default_excluded_dirs() {
        let (_dir, vault) = make_vault_with(&[
            ("notes/keep.md", "- [ ] keep me\n"),
            ("attachments/skip.md", "- [ ] skip me\n"),
        ]);
        let scan = vault.scan();
        let descs: Vec<_> = scan.tasks.iter().map(|t| t.description.clone()).collect();
        assert!(descs.contains(&"keep me".to_string()));
        assert!(!descs.contains(&"skip me".to_string()));
    }

    #[test]
    fn scan_respects_ignored_paths_from_config() {
        let dir = TempDir::new().unwrap();
        make_obsidian_dir(&dir);
        dir.child(".ft/config.toml")
            .write_str(r#"ignored_paths = ["private/"]"#)
            .unwrap();
        dir.child("public.md")
            .write_str("- [ ] public task\n")
            .unwrap();
        dir.child("private/secret.md")
            .write_str("- [ ] private task\n")
            .unwrap();

        let vault = Vault::discover(Some(dir.path().to_path_buf())).unwrap();
        let scan = vault.scan();
        let descs: Vec<_> = scan.tasks.iter().map(|t| t.description.clone()).collect();
        assert!(descs.contains(&"public task".to_string()));
        assert!(!descs.contains(&"private task".to_string()));
    }

    #[test]
    fn scan_resolves_hierarchy_per_file() {
        let (_dir, vault) = make_vault_with(&[(
            "nested.md",
            "- [ ] parent\n  - [ ] child A\n  - [ ] child B\n",
        )]);
        let scan = vault.scan();
        assert_eq!(scan.tasks.len(), 3);
        let parent = scan
            .tasks
            .iter()
            .find(|t| t.description == "parent")
            .unwrap();
        let children: Vec<_> = scan
            .tasks
            .iter()
            .filter(|t| t.parent == Some(parent.source_line))
            .collect();
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn scan_returns_relative_paths() {
        let (_dir, vault) = make_vault_with(&[("notes/sub.md", "- [ ] task\n")]);
        let scan = vault.scan();
        assert_eq!(scan.tasks.len(), 1);
        assert_eq!(
            scan.tasks[0].source_file,
            std::path::PathBuf::from("notes/sub.md")
        );
    }

    #[test]
    fn env_var_valid_vault() {
        let vault_dir = TempDir::new().unwrap();
        make_obsidian_dir(&vault_dir);

        let _guard = ENV_LOCK.lock().unwrap();
        // Ensure --vault flag is not in play (no flag passed = None)
        // We need to make sure CWD doesn't accidentally resolve to a vault.
        std::env::set_var("FT_VAULT", vault_dir.path().to_str().unwrap());

        // Pass a flag that fails so we fall through to env
        let bad_dir = TempDir::new().unwrap();
        let result = find_vault(Some(bad_dir.path().to_path_buf()));
        std::env::remove_var("FT_VAULT");

        // The env var vault should be found
        let found = result.unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            vault_dir.path().canonicalize().unwrap()
        );
    }
}
