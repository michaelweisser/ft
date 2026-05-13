//! Per-vault log of recently-opened notes.
//!
//! Backs the "show recents on empty input" picker behavior (plan 008).
//! Append-only JSONL stored under `$XDG_STATE_HOME/ft/<vault-hash>/`.
//! Best-effort: write failures are logged and swallowed — recents must
//! never break an open.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::vault::Vault;

/// Rewrite the log when it exceeds this many raw lines.
const TRIM_THRESHOLD: usize = 250;
/// Keep this many newest entries on each rewrite (cap + 50 slack →
/// post-dedupe size stays ≤ 200, matching the plan).
const TRIM_KEEP: usize = 200;

/// One open event, serialized as a single JSONL line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    /// Path relative to the vault root.
    path: PathBuf,
    opened_at: DateTime<Utc>,
}

/// Append-only "recently opened" log scoped to one vault.
///
/// Methods take `&self`; concurrency safety relies on POSIX `O_APPEND`
/// atomicity for writes smaller than `PIPE_BUF` (each entry is one
/// short JSON line, well under the limit) and atomic rename for trims.
/// A race during trim may lose entries written in the brief overlap
/// window — acceptable for a recency-only log.
#[derive(Debug, Clone)]
pub struct RecentsLog {
    /// Canonical vault root. Used to map absolute paths back to
    /// vault-relative form when recording an open.
    vault_root: PathBuf,
    /// Absolute path to the JSONL file.
    log_path: PathBuf,
}

impl RecentsLog {
    /// Construct a log with an explicit log-file path. Tests use this to
    /// point storage at a `TempDir` instead of `~/.local/state`.
    pub fn with_log_path(vault_root: PathBuf, log_path: PathBuf) -> Self {
        Self {
            vault_root,
            log_path,
        }
    }

    /// Construct the canonical per-vault log at
    /// `$XDG_STATE_HOME/ft/<vault-hash>/recents.jsonl`.
    pub fn for_vault(vault: &Vault) -> Self {
        let canonical = vault
            .path
            .canonicalize()
            .unwrap_or_else(|_| vault.path.clone());
        let hash = vault_path_hash(&canonical);
        let log_path = state_dir().join("ft").join(hash).join("recents.jsonl");
        Self::with_log_path(canonical, log_path)
    }

    /// Absolute path to the JSONL file (for diagnostics/tests).
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Record an open. Never errors: write failures are logged via
    /// `tracing::warn` and swallowed so the caller's open path is
    /// untouched by recents bookkeeping.
    pub fn record_open(&self, path: &Path) {
        let entry = Entry {
            path: self.vault_relative(path),
            opened_at: Utc::now(),
        };
        if let Err(e) = self.append(&entry) {
            warn!(
                path = %self.log_path.display(),
                error = %e,
                "recents log append failed; ignoring"
            );
            return;
        }
        if let Err(e) = self.maybe_trim() {
            warn!(
                path = %self.log_path.display(),
                error = %e,
                "recents log trim failed; ignoring"
            );
        }
    }

    /// Return up to `limit` most-recently-opened unique paths, newest
    /// first. Vault-relative. Missing or unreadable log → empty Vec
    /// (never errors).
    pub fn load_recent(&self, limit: usize) -> Vec<PathBuf> {
        if limit == 0 {
            return Vec::new();
        }
        let entries = match self.read_entries() {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    path = %self.log_path.display(),
                    error = %e,
                    "recents log read failed; treating as empty"
                );
                return Vec::new();
            }
        };
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut out: Vec<PathBuf> = Vec::with_capacity(limit);
        // File order is oldest→newest; walk back-to-front so first
        // sight of a path is the most recent occurrence.
        for entry in entries.into_iter().rev() {
            if seen.insert(entry.path.clone()) {
                out.push(entry.path);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    fn append(&self, entry: &Entry) -> std::io::Result<()> {
        if let Some(parent) = self.log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let line = serde_json::to_string(entry).expect("Entry is serializable");
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn read_entries(&self) -> std::io::Result<Vec<Entry>> {
        let file = match std::fs::File::open(&self.log_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let Ok(line) = line else {
                // I/O error mid-stream — stop here, surface what we have.
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            // Forward-compat: silently skip lines we can't parse so a
            // future schema bump doesn't brick older binaries.
            if let Ok(entry) = serde_json::from_str::<Entry>(&line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn maybe_trim(&self) -> std::io::Result<()> {
        // Count raw lines without parsing — trim is keyed on file size,
        // not post-dedupe count.
        let file = match std::fs::File::open(&self.log_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let line_count = BufReader::new(file).lines().count();
        if line_count <= TRIM_THRESHOLD {
            return Ok(());
        }
        // Above threshold: rewrite keeping only the newest TRIM_KEEP raw
        // entries. Post-dedupe length is bounded by TRIM_KEEP.
        let entries = self.read_entries()?;
        let keep_from = entries.len().saturating_sub(TRIM_KEEP);
        let kept = &entries[keep_from..];

        let tmp = self.log_path.with_extension("jsonl.tmp");
        {
            let mut f = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&tmp)?;
            for entry in kept {
                let line = serde_json::to_string(entry).expect("Entry is serializable");
                writeln!(f, "{line}")?;
            }
            f.sync_all().ok();
        }
        fs::rename(&tmp, &self.log_path)?;
        Ok(())
    }

    /// Map a caller-provided path (absolute or already-relative) to its
    /// vault-relative form. Paths outside the vault root are passed
    /// through unchanged — the caller is responsible for what it stores.
    fn vault_relative(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.strip_prefix(&self.vault_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        }
    }
}

/// Resolve the XDG state directory (`$XDG_STATE_HOME` falling back to
/// `~/.local/state`). Mirrors `vault::user_config_dir`'s XDG-everywhere
/// policy: on macOS we follow XDG rather than `~/Library/...` for
/// portability and consistency with the project's other state.
fn state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
}

/// FNV-1a 64-bit → 16-char lowercase hex. Stable across compiler versions
/// and dependency-free; not cryptographic but plenty for isolating one
/// vault's state directory from another's.
fn vault_path_hash(path: &Path) -> String {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut h: u64 = 0xcbf29ce4_84222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x00000100_000001b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use chrono::Duration;

    fn temp_log() -> (TempDir, RecentsLog) {
        let dir = TempDir::new().unwrap();
        let vault_root = dir.path().to_path_buf();
        let log_path = dir.path().join("recents.jsonl");
        (dir, RecentsLog::with_log_path(vault_root, log_path))
    }

    #[test]
    fn record_then_load_returns_path() {
        let (_dir, log) = temp_log();
        log.record_open(Path::new("note.md"));
        let got = log.load_recent(10);
        assert_eq!(got, vec![PathBuf::from("note.md")]);
    }

    #[test]
    fn duplicate_paths_dedupe_to_one_entry_newest_wins() {
        let (_dir, log) = temp_log();
        log.record_open(Path::new("a.md"));
        log.record_open(Path::new("b.md"));
        log.record_open(Path::new("a.md"));
        let got = log.load_recent(10);
        // a.md was opened most recently, so it's first; b.md follows.
        assert_eq!(
            got,
            vec![PathBuf::from("a.md"), PathBuf::from("b.md")],
            "expected a then b — a's second open promotes it"
        );
    }

    #[test]
    fn dedupe_holds_across_many_writes() {
        let (_dir, log) = temp_log();
        for _ in 0..10 {
            log.record_open(Path::new("x.md"));
        }
        log.record_open(Path::new("y.md"));
        let got = log.load_recent(10);
        assert_eq!(got, vec![PathBuf::from("y.md"), PathBuf::from("x.md")]);
    }

    #[test]
    fn newest_first_ordering() {
        let (_dir, log) = temp_log();
        for i in 0..5 {
            log.record_open(&PathBuf::from(format!("note-{i}.md")));
        }
        let got = log.load_recent(10);
        let names: Vec<String> = got
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "note-4.md",
                "note-3.md",
                "note-2.md",
                "note-1.md",
                "note-0.md"
            ]
        );
    }

    #[test]
    fn limit_truncates_load() {
        let (_dir, log) = temp_log();
        for i in 0..20 {
            log.record_open(&PathBuf::from(format!("n{i:02}.md")));
        }
        assert_eq!(log.load_recent(0).len(), 0);
        assert_eq!(log.load_recent(3).len(), 3);
        assert_eq!(log.load_recent(100).len(), 20);
    }

    #[test]
    fn missing_log_returns_empty() {
        let dir = TempDir::new().unwrap();
        let log = RecentsLog::with_log_path(
            dir.path().to_path_buf(),
            dir.path().join("does-not-exist.jsonl"),
        );
        assert!(log.load_recent(10).is_empty());
    }

    #[test]
    fn malformed_lines_skipped() {
        let (_dir, log) = temp_log();
        // Manually plant a mix of good and bad lines.
        let now = Utc::now();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log.log_path())
            .unwrap();
        writeln!(f, "not json at all").unwrap();
        let good = serde_json::to_string(&Entry {
            path: PathBuf::from("kept.md"),
            opened_at: now,
        })
        .unwrap();
        writeln!(f, "{good}").unwrap();
        writeln!(f, "{{\"path\": \"missing-opened-at.md\"}}").unwrap();
        writeln!(f).unwrap();
        let newer = serde_json::to_string(&Entry {
            path: PathBuf::from("also-kept.md"),
            opened_at: now + Duration::seconds(1),
        })
        .unwrap();
        writeln!(f, "{newer}").unwrap();
        drop(f);

        let got = log.load_recent(10);
        assert_eq!(
            got,
            vec![PathBuf::from("also-kept.md"), PathBuf::from("kept.md")]
        );
    }

    #[test]
    fn trim_keeps_newest_after_threshold() {
        let (_dir, log) = temp_log();
        // Drive raw line count above TRIM_THRESHOLD using distinct paths
        // so no dedupe collapses entries pre-trim.
        let total = TRIM_THRESHOLD + 5;
        for i in 0..total {
            log.record_open(&PathBuf::from(format!("note-{i:04}.md")));
        }

        // After at least one trim the raw line count must drop back
        // toward TRIM_KEEP — the cap is "post-trim size", and between
        // trims the file may grow up to TRIM_THRESHOLD before the next
        // rewrite. Either invariant holds: total writes (which would
        // be the pre-trim ceiling) is unreachable; pre-trim window is
        // capped by the slack, so the file stays ≤ TRIM_THRESHOLD.
        let raw_lines = std::fs::read_to_string(log.log_path()).unwrap();
        let line_count = raw_lines.lines().filter(|l| !l.is_empty()).count();
        assert!(
            line_count <= TRIM_THRESHOLD,
            "raw line count {line_count} should be ≤ TRIM_THRESHOLD={TRIM_THRESHOLD}"
        );
        assert!(
            line_count < total,
            "raw line count {line_count} should be below total writes {total} — trim must have run"
        );

        // Newest entry survived; oldest did not.
        let got = log.load_recent(TRIM_KEEP);
        let names: Vec<String> = got
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names[0], format!("note-{:04}.md", total - 1));
        assert!(!names.contains(&"note-0000.md".to_string()));
    }

    #[test]
    fn record_open_on_unwritable_dir_does_not_panic() {
        // Point the log at a path under a non-existent ancestor whose
        // creation will succeed; then make the parent read-only and try
        // to write. On platforms where chmod doesn't block root, the
        // write may succeed — that's fine, the contract is "no panic".
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("locked");
        std::fs::create_dir_all(&parent).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&parent).unwrap().permissions();
            p.set_mode(0o500); // r-x, no write
            std::fs::set_permissions(&parent, p).unwrap();
        }
        let log = RecentsLog::with_log_path(
            dir.path().to_path_buf(),
            parent.join("nested").join("recents.jsonl"),
        );

        // Must not panic regardless of whether the underlying write
        // succeeds or not.
        log.record_open(Path::new("x.md"));

        #[cfg(unix)]
        {
            // Restore so TempDir can clean up.
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&parent).unwrap().permissions();
            p.set_mode(0o700);
            std::fs::set_permissions(&parent, p).unwrap();
        }
    }

    #[test]
    fn absolute_paths_stored_as_vault_relative() {
        let dir = TempDir::new().unwrap();
        let vault_root = dir.path().to_path_buf();
        let log_path = dir.path().join("recents.jsonl");
        let log = RecentsLog::with_log_path(vault_root.clone(), log_path);

        log.record_open(&vault_root.join("sub").join("note.md"));
        let got = log.load_recent(1);
        assert_eq!(got, vec![PathBuf::from("sub/note.md")]);
    }

    #[test]
    fn paths_outside_vault_stored_as_given() {
        let dir = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let log_path = dir.path().join("recents.jsonl");
        let log = RecentsLog::with_log_path(dir.path().to_path_buf(), log_path);

        let outsider = other.path().join("foreign.md");
        log.record_open(&outsider);
        let got = log.load_recent(1);
        assert_eq!(got, vec![outsider]);
    }

    #[test]
    fn vault_path_hash_is_deterministic_and_distinct() {
        let a = vault_path_hash(Path::new("/Users/me/Vault"));
        let b = vault_path_hash(Path::new("/Users/me/Vault"));
        let c = vault_path_hash(Path::new("/Users/me/Other"));
        assert_eq!(a, b, "hash must be deterministic");
        assert_ne!(a, c, "different paths must hash differently");
        assert_eq!(a.len(), 16, "hash is 16-char hex");
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
    }
}
