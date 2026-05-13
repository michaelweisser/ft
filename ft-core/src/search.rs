//! Fuzzy file + heading search across a vault.
//!
//! See plan 005 for the design rationale. The capability ships as a pure
//! library function ([`fuzzy_find`]) plus an inherent [`Vault::fuzzy_find`]
//! convenience that delegates to it.
//!
//! ## Query language
//!
//! - `text` — fuzzy-match filenames only
//! - `text#heading` — fuzzy-match filenames, then within each candidate
//!   fuzzy-match headings; results carry a [`Heading`] payload
//! - `#heading` — fuzzy-match headings across the whole vault
//! - `text#` — same as `text` (trailing `#` is a no-op so progressive
//!   typing doesn't surface an error mid-keystroke)
//! - Empty query → empty result, no error
//!
//! ## Scoring
//!
//! Scores come from [`nucleo_matcher`]. The path matcher is configured with
//! `Config::DEFAULT.match_paths()` so basename matches naturally outrank
//! directory-component matches. We add a small manual bonus to level-1
//! headings on top of the nucleo score so `# Big Topic` outranks a deeply
//! nested `###### Big Topic` when both have equal fuzzy quality.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};
use rayon::prelude::*;

use crate::markdown::{extract_headings, Heading};
use crate::recents::RecentsLog;
use crate::vault::Vault;

// ── public types ─────────────────────────────────────────────────────────────

/// A parsed query of the form `file_part[#heading_part]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub file_part: String,
    pub heading_part: Option<String>,
}

impl Query {
    /// Split `input` on the first `#`, trim each side, and yield a parsed
    /// query. A trailing `#` (empty heading) is preserved as
    /// `Some(String::new())`-but-then-treated-as-`None` so the caller's
    /// behavior matches "no heading constraint" — see the acceptance
    /// criterion `text#` semantics. Internal whitespace is preserved
    /// because nucleo treats spaces as separator chars in its multi-atom
    /// patterns (e.g. `gen consid` becomes two atoms).
    pub fn parse(input: &str) -> Self {
        match input.split_once('#') {
            Some((file, heading)) => {
                let file = file.trim().to_string();
                let heading = heading.trim().to_string();
                Query {
                    file_part: file,
                    heading_part: (!heading.is_empty()).then_some(heading),
                }
            }
            None => Query {
                file_part: input.trim().to_string(),
                heading_part: None,
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.file_part.is_empty() && self.heading_part.is_none()
    }
}

/// A single ranked result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Path relative to the vault root.
    pub path: PathBuf,
    pub file_score: u32,
    pub heading: Option<Heading>,
    pub heading_score: Option<u32>,
    /// Sum of `file_score` and `heading_score`, used as the canonical
    /// ranking key. Files without a heading hit (when the query asked for
    /// one) are filtered out before this is computed.
    pub total_score: u32,
}

/// Caller-facing configuration.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Maximum number of hits to return after ranking.
    pub limit: usize,
    /// If `true`, extract and score headings even when the query has no
    /// `heading_part`. When the query *does* have a `heading_part`, this
    /// flag is effectively forced on by [`fuzzy_find`].
    pub include_headings: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 25,
            include_headings: false,
        }
    }
}

// ── algorithm ────────────────────────────────────────────────────────────────

/// Run a fuzzy search against `vault`.
///
/// Two-stage filter: (1) score every filename against `query.file_part`
/// using a path-aware matcher and discard non-matches, (2) when headings
/// are in play, read each survivor in parallel, extract its headings,
/// and pick the best per-file heading score. Combined score = file +
/// heading; results are sorted desc with a lexicographic path tiebreaker
/// so equal scores don't reshuffle between identical queries.
pub fn fuzzy_find(vault: &Vault, query: &Query, opts: SearchOptions) -> Vec<Hit> {
    if query.is_empty() {
        return Vec::new();
    }

    let want_headings = opts.include_headings || query.heading_part.is_some();
    let files = vault.markdown_files();

    // Stage 1: filename matching.
    let file_matches: Vec<(PathBuf, u32)> = if query.file_part.is_empty() {
        // Heading-only query (`#heading`) — every file is a candidate;
        // assign a neutral file_score of 0 so the combined ranking is
        // driven entirely by the heading match.
        files
            .into_iter()
            .map(|p| (rel(&p, &vault.path), 0u32))
            .collect()
    } else {
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(&query.file_part, CaseMatching::Ignore, Normalization::Smart);
        let mut buf: Vec<char> = Vec::new();
        files
            .into_iter()
            .filter_map(|p| {
                let rel = rel(&p, &vault.path);
                let s = rel.to_string_lossy();
                let haystack = Utf32Str::new(&s, &mut buf);
                pattern.score(haystack, &mut matcher).map(|sc| (rel, sc))
            })
            .collect()
    };

    if !want_headings {
        return rank_and_truncate(
            file_matches
                .into_iter()
                .map(|(path, file_score)| Hit {
                    path,
                    file_score,
                    heading: None,
                    heading_score: None,
                    total_score: file_score,
                })
                .collect(),
            opts.limit,
        );
    }

    // Stage 2: heading extraction in parallel (I/O bound) — but only on
    // files that survived stage 1. Then score headings serially.
    let with_headings: Vec<(PathBuf, u32, Vec<Heading>)> = file_matches
        .par_iter()
        .map(|(path, file_score)| {
            let abs = vault.path.join(path);
            let headings = match std::fs::read_to_string(&abs) {
                Ok(content) => extract_headings(&content),
                Err(_) => Vec::new(),
            };
            (path.clone(), *file_score, headings)
        })
        .collect();

    let heading_pattern = query
        .heading_part
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|p| Pattern::parse(p, CaseMatching::Ignore, Normalization::Smart));

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf: Vec<char> = Vec::new();
    let mut hits: Vec<Hit> = Vec::new();
    for (path, file_score, headings) in with_headings {
        let best = match &heading_pattern {
            Some(pat) => best_heading_match(pat, &headings, &mut matcher, &mut buf),
            // include_headings=true with no heading_part: surface the
            // first heading per file (if any) as a navigation aid.
            None => headings.into_iter().next().map(|h| (h, 0u32)),
        };

        // If the query asked for a specific heading and the file has no
        // matching one, drop the file from the results entirely.
        if heading_pattern.is_some() && best.is_none() {
            continue;
        }

        let (heading, heading_score) = match best {
            Some((h, s)) => (Some(h), Some(s)),
            None => (None, None),
        };
        let level_bonus = heading.as_ref().map(|h| level_bonus(h.level)).unwrap_or(0);
        let total_score = file_score
            .saturating_add(heading_score.unwrap_or(0))
            .saturating_add(level_bonus);
        hits.push(Hit {
            path,
            file_score,
            heading,
            heading_score,
            total_score,
        });
    }

    rank_and_truncate(hits, opts.limit)
}

/// Score every heading in `headings` against `pattern`, return the
/// (heading, score) pair with the highest score, or `None` if no heading
/// matches.
fn best_heading_match(
    pattern: &Pattern,
    headings: &[Heading],
    matcher: &mut Matcher,
    buf: &mut Vec<char>,
) -> Option<(Heading, u32)> {
    let mut best: Option<(Heading, u32)> = None;
    for h in headings {
        let haystack = Utf32Str::new(&h.text, buf);
        if let Some(score) = pattern.score(haystack, matcher) {
            match &best {
                Some((_, current)) if *current >= score => {}
                _ => best = Some((h.clone(), score)),
            }
        }
    }
    best
}

/// Sort hits by `total_score` desc, with lexicographic path asc as the
/// stable tiebreaker, then take the top `limit`.
fn rank_and_truncate(mut hits: Vec<Hit>, limit: usize) -> Vec<Hit> {
    hits.sort_by(|a, b| {
        b.total_score
            .cmp(&a.total_score)
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.truncate(limit);
    hits
}

/// Strip the vault prefix so all returned paths are vault-relative.
fn rel(p: &Path, vault_root: &Path) -> PathBuf {
    p.strip_prefix(vault_root).unwrap_or(p).to_path_buf()
}

/// Small bonus that boosts level-1 headings over deeper ones when their
/// nucleo scores tie. Empirical numbers — tuned so a level-1 heading match
/// barely edges out a level-6 match of identical fuzzy quality but doesn't
/// dominate a clearly-better deeper match.
fn level_bonus(level: u8) -> u32 {
    match level {
        1 => 10,
        2 => 6,
        3 => 3,
        _ => 0,
    }
}

// ── recents merge ────────────────────────────────────────────────────────────

/// Build the "empty-input" picker list: opens-first, mtime-tail.
///
/// 1. Pull up to `limit * 2` paths from `recents` (oversample so paths
///    that no longer exist in the vault don't shrink the result below
///    `limit`).
/// 2. Walk the vault for the current markdown file set with mtimes.
/// 3. Emit opens first in recency order, filtered against the live file
///    set so deleted notes don't appear.
/// 4. Fill the tail with mtime-ordered files not already taken.
///
/// `Hit.path` is vault-relative for consistency with [`fuzzy_find`].
/// All scores are zero and `heading` is `None` — the picker renders
/// these rows verbatim, with no match highlighting.
pub fn recent_hits(vault: &Vault, recents: &RecentsLog, limit: usize) -> Vec<Hit> {
    if limit == 0 {
        return Vec::new();
    }

    let opens = recents.load_recent(limit.saturating_mul(2));

    let files_with_mtime: Vec<(PathBuf, std::time::SystemTime)> = vault
        .markdown_files_with_mtime()
        .into_iter()
        .map(|(abs, mt)| (rel(&abs, &vault.path), mt))
        .collect();
    let file_set: HashSet<PathBuf> = files_with_mtime.iter().map(|(p, _)| p.clone()).collect();

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<Hit> = Vec::with_capacity(limit);

    for path in opens {
        if !file_set.contains(&path) {
            continue;
        }
        if seen.insert(path.clone()) {
            out.push(recent_hit(path));
            if out.len() >= limit {
                return out;
            }
        }
    }

    let mut tail: Vec<(PathBuf, std::time::SystemTime)> = files_with_mtime
        .into_iter()
        .filter(|(p, _)| !seen.contains(p))
        .collect();
    // Newest mtime first; stable lexicographic tiebreaker keeps results
    // deterministic when two files share an mtime (common in tests).
    tail.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    for (path, _mt) in tail {
        out.push(recent_hit(path));
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn recent_hit(path: PathBuf) -> Hit {
    Hit {
        path,
        file_score: 0,
        heading: None,
        heading_score: None,
        total_score: 0,
    }
}

// ── Vault inherent ───────────────────────────────────────────────────────────

impl Vault {
    /// Convenience wrapper for [`crate::search::fuzzy_find`].
    pub fn fuzzy_find(&self, query: &Query, opts: SearchOptions) -> Vec<Hit> {
        fuzzy_find(self, query, opts)
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use std::fs;

    // ── Query parsing ───────────────────────────────────────────────

    #[test]
    fn query_parse_no_hash_is_file_only() {
        let q = Query::parse("gen consid");
        assert_eq!(q.file_part, "gen consid");
        assert_eq!(q.heading_part, None);
    }

    #[test]
    fn query_parse_file_and_heading() {
        let q = Query::parse("gen consid#Firs");
        assert_eq!(q.file_part, "gen consid");
        assert_eq!(q.heading_part, Some("Firs".into()));
    }

    #[test]
    fn query_parse_heading_only() {
        let q = Query::parse("#Firs");
        assert_eq!(q.file_part, "");
        assert_eq!(q.heading_part, Some("Firs".into()));
    }

    #[test]
    fn query_parse_trailing_hash_drops_heading() {
        let q = Query::parse("foo#");
        assert_eq!(q.file_part, "foo");
        assert_eq!(q.heading_part, None);
    }

    #[test]
    fn query_parse_trims_outer_whitespace_keeps_inner() {
        let q = Query::parse("  foo bar  #  baz qux  ");
        assert_eq!(q.file_part, "foo bar");
        assert_eq!(q.heading_part, Some("baz qux".into()));
    }

    #[test]
    fn query_parse_empty_string() {
        let q = Query::parse("");
        assert!(q.is_empty());
    }

    // ── fuzzy_find against synthetic vaults ─────────────────────────

    /// Build a vault with the given (relative path, body) pairs.
    fn make_vault(files: &[(&str, &str)]) -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("vault");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        for (rel, body) in files {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, body).unwrap();
        }
        let vault = Vault::discover(Some(root)).unwrap();
        (dir, vault)
    }

    #[test]
    fn file_only_query_ranks_filename_matches() {
        let (_dir, vault) = make_vault(&[
            ("General Considerations about Food.md", "# X\n"),
            ("initial general considerations.md", "# Y\n"),
            ("unrelated.md", "# Z\n"),
        ]);
        let q = Query::parse("gen consid");
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());

        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("General Considerations")),
            "expected the General Considerations file in results: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.contains("initial general considerations")),
            "expected the initial-general file in results: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("unrelated")),
            "non-matching file should be excluded: {names:?}"
        );
        for h in &hits {
            assert!(h.file_score > 0);
            assert!(h.heading.is_none());
        }
    }

    #[test]
    fn file_and_heading_query_filters_to_files_with_matching_heading() {
        let (_dir, vault) = make_vault(&[
            (
                "General Considerations about Food.md",
                "# Intro\n## Second Try\n",
            ),
            (
                "initial general considerations.md",
                "# Overview\n### First Try\n",
            ),
        ]);
        let q = Query::parse("gen consid#Firs");
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());

        // The "initial general considerations" file has the heading
        // "First Try" which fuzzy-matches "Firs"; the other file's
        // "Second Try" does not match "Firs" as well — but it might
        // still produce a low score. The key acceptance is that the
        // top hit must be the file with the closer heading match.
        let top = &hits[0];
        assert!(
            top.path
                .to_string_lossy()
                .contains("initial general considerations"),
            "top hit should be the file with 'First Try': got {:?}",
            top.path
        );
        let heading = top.heading.as_ref().expect("heading payload missing");
        assert_eq!(heading.text, "First Try");
        assert_eq!(heading.level, 3);
        assert!(heading.line >= 2);
        assert!(top.heading_score.unwrap() > 0);
        assert_eq!(
            top.total_score,
            top.file_score + top.heading_score.unwrap() + level_bonus(heading.level)
        );
    }

    #[test]
    fn heading_only_query_searches_every_file() {
        let (_dir, vault) = make_vault(&[
            ("a.md", "# Alpha\n# Other\n"),
            ("b.md", "# Beta\n## Alpha Variant\n"),
            ("c.md", "no heading here\n"),
        ]);
        let q = Query::parse("#Alpha");
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());

        // Two files have an "Alpha"-ish heading. The third drops out.
        assert_eq!(hits.len(), 2);
        for h in &hits {
            assert!(h.heading.is_some());
            assert!(h.heading_score.is_some());
        }
        let texts: Vec<String> = hits
            .iter()
            .map(|h| h.heading.as_ref().unwrap().text.clone())
            .collect();
        assert!(texts.iter().any(|t| t.contains("Alpha")));
    }

    #[test]
    fn no_match_returns_empty() {
        let (_dir, vault) = make_vault(&[("foo.md", "# H\n")]);
        let q = Query::parse("zzzzzzz");
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());
        assert!(hits.is_empty());
    }

    #[test]
    fn empty_query_returns_empty() {
        let (_dir, vault) = make_vault(&[("foo.md", "# H\n")]);
        let q = Query::parse("");
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());
        assert!(hits.is_empty());
    }

    #[test]
    fn limit_truncates_results() {
        let pairs: Vec<(String, String)> = (0..50)
            .map(|i| (format!("note-{i:02}.md"), "# H\n".to_string()))
            .collect();
        let refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_str()))
            .collect();
        let (_dir, vault) = make_vault(&refs);
        let q = Query::parse("note");
        let opts = SearchOptions {
            limit: 5,
            include_headings: false,
        };
        let hits = fuzzy_find(&vault, &q, opts);
        assert_eq!(hits.len(), 5);
    }

    #[test]
    fn tiebreaker_is_path_lexicographic() {
        // Identical filename pattern → equal nucleo scores → falls back
        // to lexicographic path order.
        let (_dir, vault) = make_vault(&[
            ("zeta.md", "# H\n"),
            ("alpha.md", "# H\n"),
            ("mike.md", "# H\n"),
        ]);
        let q = Query::parse(".md");
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());
        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // All three have equal scores, so order should be alpha, mike, zeta.
        // (If nucleo doesn't tie them exactly, this test fails — adjust.)
        assert_eq!(names, vec!["alpha.md", "mike.md", "zeta.md"]);
    }

    #[test]
    fn include_headings_without_heading_part_attaches_first_heading() {
        let (_dir, vault) = make_vault(&[("notes.md", "# Top Heading\n## Sub\nbody text\n")]);
        let q = Query::parse("notes");
        let opts = SearchOptions {
            limit: 10,
            include_headings: true,
        };
        let hits = fuzzy_find(&vault, &q, opts);
        assert_eq!(hits.len(), 1);
        let h = hits[0].heading.as_ref().expect("heading attached");
        assert_eq!(h.text, "Top Heading");
        assert_eq!(h.level, 1);
    }

    // ── perf budgets (gated; run with FT_PERF_TESTS=1) ─────────────────

    fn perf_enabled() -> bool {
        std::env::var("FT_PERF_TESTS").as_deref() == Ok("1")
    }

    /// 5k synthetic notes with realistic-ish structure (one body para +
    /// a couple of headings) so heading extraction has actual work to do.
    /// Setup time is amortized: the test creates the vault once and runs
    /// multiple queries against it.
    fn synthetic_5k_vault() -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("vault");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        for i in 0..5000u32 {
            let folder = match i % 5 {
                0 => "Areas",
                1 => "Projects",
                2 => "Journal",
                3 => "Archive",
                _ => "Inbox",
            };
            let topic = match i % 7 {
                0 => "review",
                1 => "design",
                2 => "research",
                3 => "report",
                4 => "consideration",
                5 => "follow-up",
                _ => "draft",
            };
            let dir_path = root.join(folder);
            fs::create_dir_all(&dir_path).unwrap();
            let body =
                format!("# Note {i} ({topic})\n## First Try\nbody paragraph\n## Second pass\n",);
            // Topic baked into the filename so file-only fuzzy queries
            // (`consid`, `design`, …) actually have something to match.
            fs::write(dir_path.join(format!("note-{i:05}-{topic}.md")), body).unwrap();
        }
        let vault = Vault::discover(Some(root)).unwrap();
        (dir, vault)
    }

    #[test]
    fn perf_file_only_query_under_budget_on_5k_vault() {
        if !perf_enabled() {
            return;
        }
        let (_dir, vault) = synthetic_5k_vault();

        // Cold-ish: first query touches no file content (file-only), so
        // it's pure path matching across 5k filenames.
        let q = Query::parse("consid");
        let start = std::time::Instant::now();
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());
        let cold = start.elapsed();
        assert!(
            !hits.is_empty(),
            "expected matches for `consid` against 5k vault"
        );

        // Warm: same query again (OS file cache + lazy paths are warm).
        let start = std::time::Instant::now();
        let _ = fuzzy_find(&vault, &q, SearchOptions::default());
        let warm = start.elapsed();

        // Plan budgets: cold <100ms / warm <50ms in release. Allow 5x for
        // debug builds and slow CI; perf tests are the one place where
        // running with --release matters and is documented in the plan.
        assert!(
            cold.as_millis() < 500,
            "cold file-only query took {cold:?}; budget 100ms (5x debug headroom)"
        );
        assert!(
            warm.as_millis() < 250,
            "warm file-only query took {warm:?}; budget 50ms (5x debug headroom)"
        );
    }

    #[test]
    fn perf_file_and_heading_query_under_budget_on_5k_vault() {
        if !perf_enabled() {
            return;
        }
        let (_dir, vault) = synthetic_5k_vault();

        // file+heading query forces the rayon-parallel heading extraction
        // path on every filename match — the hot loop in fuzzy_find.
        let q = Query::parse("consid#First");
        let start = std::time::Instant::now();
        let hits = fuzzy_find(&vault, &q, SearchOptions::default());
        let cold = start.elapsed();
        assert!(!hits.is_empty(), "expected hits for `consid#First`");
        assert!(hits.iter().all(|h| h.heading.is_some()));

        let start = std::time::Instant::now();
        let _ = fuzzy_find(&vault, &q, SearchOptions::default());
        let warm = start.elapsed();

        assert!(
            cold.as_millis() < 500,
            "cold file+heading query took {cold:?}; budget 100ms (5x debug headroom)"
        );
        assert!(
            warm.as_millis() < 250,
            "warm file+heading query took {warm:?}; budget 50ms (5x debug headroom)"
        );
    }

    // ── recent_hits ─────────────────────────────────────────────────

    fn make_recents_for(vault: &Vault, tmp: &TempDir) -> RecentsLog {
        let log_path = tmp.path().join("recents.jsonl");
        RecentsLog::with_log_path(vault.path.clone(), log_path)
    }

    #[test]
    fn recent_hits_mtime_only_returns_files_newest_first() {
        let (dir, vault) = make_vault(&[
            ("old.md", "# old\n"),
            ("mid.md", "# mid\n"),
            ("new.md", "# new\n"),
        ]);
        // Force distinct mtimes by writing in order with sleeps short
        // enough to keep the test fast but long enough to register on
        // filesystems with 1s resolution.
        let now = std::time::SystemTime::now();
        for (rel, offset_secs) in [("old.md", 0u64), ("mid.md", 5), ("new.md", 10)] {
            let abs = vault.path.join(rel);
            let mt = now + std::time::Duration::from_secs(offset_secs);
            // Use filetime if available; otherwise just rewrite which
            // bumps mtime to "now" — order then comes from write order.
            std::fs::write(&abs, format!("# {rel}\n")).unwrap();
            // Best-effort: pin mtime via stable std API.
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&abs) {
                let _ = f.set_times(std::fs::FileTimes::new().set_modified(mt));
            }
        }

        let recents = make_recents_for(&vault, &dir);
        let hits = recent_hits(&vault, &recents, 25);
        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.to_string_lossy().into_owned())
            .collect();

        // On unix we expect deterministic mtime order; on other
        // platforms we at least assert the set is right.
        assert_eq!(names.len(), 3);
        let set: HashSet<&str> = names.iter().map(String::as_str).collect();
        assert!(set.contains("new.md"));
        assert!(set.contains("mid.md"));
        assert!(set.contains("old.md"));
        #[cfg(unix)]
        assert_eq!(names, vec!["new.md", "mid.md", "old.md"]);

        for h in &hits {
            assert!(h.heading.is_none());
            assert_eq!(h.file_score, 0);
            assert_eq!(h.total_score, 0);
        }
    }

    #[test]
    fn recent_hits_opens_only_returns_log_order() {
        let (dir, vault) = make_vault(&[("a.md", "# a\n"), ("b.md", "# b\n"), ("c.md", "# c\n")]);
        let recents = make_recents_for(&vault, &dir);
        recents.record_open(Path::new("a.md"));
        recents.record_open(Path::new("c.md"));

        let hits = recent_hits(&vault, &recents, 2);
        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.to_string_lossy().into_owned())
            .collect();
        // c was opened most recently, so it's first; a follows.
        assert_eq!(names, vec!["c.md", "a.md"]);
    }

    #[test]
    fn recent_hits_merges_opens_first_then_mtime_tail() {
        let (dir, vault) = make_vault(&[
            ("a.md", "# a\n"),
            ("b.md", "# b\n"),
            ("c.md", "# c\n"),
            ("d.md", "# d\n"),
        ]);
        let recents = make_recents_for(&vault, &dir);
        // Open only a.md — it must appear first, regardless of mtime.
        recents.record_open(Path::new("a.md"));

        let hits = recent_hits(&vault, &recents, 25);
        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names[0], "a.md", "opened file leads the list");
        assert_eq!(names.len(), 4, "all files appear");
        // a.md must appear exactly once even though it could also match
        // the mtime tail.
        let a_count = names.iter().filter(|n| *n == "a.md").count();
        assert_eq!(a_count, 1, "no duplication between opens and mtime tail");
    }

    #[test]
    fn recent_hits_drops_deleted_paths_from_log() {
        let (dir, vault) = make_vault(&[("alive.md", "# alive\n")]);
        let recents = make_recents_for(&vault, &dir);
        recents.record_open(Path::new("ghost.md"));
        recents.record_open(Path::new("alive.md"));

        let hits = recent_hits(&vault, &recents, 25);
        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|n| n == "ghost.md"),
            "deleted file should not surface; got {names:?}"
        );
        assert!(names.iter().any(|n| n == "alive.md"));
    }

    #[test]
    fn recent_hits_honors_limit() {
        let (dir, vault) = make_vault(&[
            ("a.md", "# a\n"),
            ("b.md", "# b\n"),
            ("c.md", "# c\n"),
            ("d.md", "# d\n"),
            ("e.md", "# e\n"),
        ]);
        let recents = make_recents_for(&vault, &dir);
        recents.record_open(Path::new("a.md"));
        recents.record_open(Path::new("b.md"));
        let hits = recent_hits(&vault, &recents, 3);
        assert_eq!(hits.len(), 3, "limit must be honored across the merge");

        assert!(recent_hits(&vault, &recents, 0).is_empty());
    }

    #[test]
    fn recent_hits_dedup_keeps_open_position() {
        // Open a.md, then b.md. b.md is also mtime-fresh; opens-first
        // must still place a (opened earlier) ahead of b? No — b was
        // opened *later*, so it's first in load_recent (newest-first).
        // The dedupe rule is: a path that appears in the opens slice
        // doesn't reappear in the mtime tail.
        let (dir, vault) = make_vault(&[("a.md", "# a\n"), ("b.md", "# b\n")]);
        let recents = make_recents_for(&vault, &dir);
        recents.record_open(Path::new("a.md"));
        recents.record_open(Path::new("b.md"));

        let hits = recent_hits(&vault, &recents, 10);
        let names: Vec<String> = hits
            .iter()
            .map(|h| h.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["b.md", "a.md"]);
    }

    #[test]
    fn recent_hits_empty_vault_empty_log_returns_empty() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("vault");
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        let vault = Vault::discover(Some(root)).unwrap();
        let recents = make_recents_for(&vault, &dir);
        assert!(recent_hits(&vault, &recents, 25).is_empty());
    }
}
