//! Link extraction from markdown content.
//!
//! Parses three link forms in document order, with byte ranges that
//! round-trip against the source:
//!
//! - **Wikilinks**: `[[Target]]`, `[[Target|Display]]`,
//!   `[[Target#Anchor]]`, `[[Target#Anchor|Display]]`
//! - **Embeds**: any of the above prefixed with `!`
//!   (`![[Target]]`, `![[image.png]]`, ...)
//! - **Markdown links**: `[Display](href)` and `![Display](href)` for
//!   embed-form image/file links, where `href` is a vault path (we only
//!   record edges for relative paths — external URLs `http://`,
//!   `https://`, `mailto:`, `obsidian://` are *not* edges).
//!
//! Skips frontmatter, fenced code blocks, indented code blocks (via the
//! shared [`crate::markdown::LineSkipState`]), and inline code spans
//! (single/double/triple backtick runs within a line).
//!
//! Reference-style markdown links (`[text][ref]` plus `[ref]: url`) are
//! out of scope for v1 — uncommon in Obsidian vaults.

use crate::graph::LinkForm;
use crate::markdown::LineSkipState;

/// Per-occurrence link record returned by [`extract_links`]. Resolution
/// to a [`crate::graph::LinkTarget`] happens later, in
/// [`crate::graph::Graph::insert_edges_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    pub form: LinkForm,
    pub is_embed: bool,
    /// Byte range in the source content. `&content[byte_range] == raw_text`.
    pub byte_range: std::ops::Range<usize>,
    /// 1-indexed source line.
    pub line: usize,
    pub raw_text: String,
    /// Pre-pipe, pre-anchor target text. For markdown links this is the
    /// URL-decoded href (with any `.md` suffix preserved verbatim).
    pub target_text: String,
    pub anchor: Option<String>,
    pub display: Option<String>,
}

/// Extract every link occurrence from `content` in document order.
///
/// Lines inside frontmatter / fenced code / indented code are skipped
/// entirely; inline code spans within a content line are skipped at the
/// span level (the line still contributes other links outside the span).
pub fn extract_links(content: &str) -> Vec<RawLink> {
    let mut out = Vec::new();
    let mut state = LineSkipState::new();

    // Track the byte offset of each line's first byte so we can convert
    // intra-line offsets to file-byte ranges.
    let mut line_start = 0usize;
    let mut lineno = 0usize;

    for line in content.split_inclusive('\n') {
        lineno += 1;
        // `line` includes the trailing '\n' if present. The skip
        // helpers want the line without it; passing `line` whole is
        // also fine because they trim_end where needed.
        let line_no_newline = line.strip_suffix('\n').unwrap_or(line);

        if state.skip_line(line_no_newline) {
            line_start += line.len();
            continue;
        }

        scan_line(line_no_newline, line_start, lineno, &mut out);
        line_start += line.len();
    }

    out
}

/// Scan a single content line for links, emitting [`RawLink`] records
/// with file-byte ranges (computed from `line_start_offset`).
fn scan_line(line: &str, line_start_offset: usize, lineno: usize, out: &mut Vec<RawLink>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        // Inline code span: skip until the matching closing run.
        if b == b'`' {
            let run_start = i;
            let mut run_len = 0usize;
            while i < bytes.len() && bytes[i] == b'`' {
                run_len += 1;
                i += 1;
            }
            // Find closing run of the same length on the same line.
            let mut j = i;
            while j < bytes.len() {
                if bytes[j] == b'`' {
                    let close_start = j;
                    let mut close_len = 0usize;
                    while j < bytes.len() && bytes[j] == b'`' {
                        close_len += 1;
                        j += 1;
                    }
                    if close_len == run_len {
                        i = j;
                        break;
                    }
                    // Mismatched run length — keep searching.
                    let _ = close_start;
                    continue;
                }
                j += 1;
            }
            if j >= bytes.len() {
                // Unterminated code span on this line — per CommonMark
                // the backticks are literal text, not a code span.
                // Resume scanning from just past the opening run so any
                // following links still surface.
                i = run_start + run_len;
            }
            continue;
        }

        // Embed-or-link prefix: `![[...]]` or `![alt](url)`.
        if b == b'!' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next == b'[' && i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                if let Some(end) = parse_wikilink(bytes, i + 1) {
                    let span = i..end;
                    push_wikilink(line, line_start_offset, lineno, span, true, out);
                    i = end;
                    continue;
                }
            } else if next == b'[' {
                if let Some((end, link)) = parse_md_link(line, i + 1, true) {
                    push_md_link(line_start_offset, lineno, i..end, link, out);
                    i = end;
                    continue;
                }
            }
            i += 1;
            continue;
        }

        if b == b'[' {
            // Wikilink `[[...]]`?
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                if let Some(end) = parse_wikilink(bytes, i) {
                    let span = i..end;
                    push_wikilink(line, line_start_offset, lineno, span, false, out);
                    i = end;
                    continue;
                }
            }
            // Markdown link `[text](url)`?
            if let Some((end, link)) = parse_md_link(line, i, false) {
                push_md_link(line_start_offset, lineno, i..end, link, out);
                i = end;
                continue;
            }
            i += 1;
            continue;
        }

        i += 1;
    }
}

/// Find the byte offset just past the closing `]]` of a wikilink whose
/// opening `[[` starts at `start`. Returns `None` if the line ends
/// before the close, or the wikilink body is empty.
///
/// Embeds (`![[...]]`) call this with `start` pointing at the `[[`,
/// not the `!`.
fn parse_wikilink(bytes: &[u8], start: usize) -> Option<usize> {
    debug_assert!(start + 1 < bytes.len() && bytes[start] == b'[' && bytes[start + 1] == b'[');
    let body_start = start + 2;
    let mut i = body_start;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            if i == body_start {
                return None; // empty body, not a link
            }
            return Some(i + 2);
        }
        // Newlines inside `[[...]]` would mean the link spans lines —
        // this scanner is line-scoped, so the caller's outer loop never
        // gives us a line containing `\n`. Defensive: bail if we see one.
        if bytes[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

#[derive(Debug)]
struct MdLink {
    display: String,
    href: String,
    raw_text: String,
}

/// Try to parse a `[display](href)` (or `![display](href)`) pair where
/// the opening `[` is at byte `start` of `line`. The `is_embed` flag
/// adjusts the recorded `raw_text` only — embed detection at the
/// caller level decides which `EdgeKind` variant to build.
///
/// External URLs (`http://`, `https://`, `mailto:`, `obsidian://`) are
/// rejected here so they don't become edges. Empty href rejected.
fn parse_md_link(line: &str, start: usize, is_embed: bool) -> Option<(usize, MdLink)> {
    let bytes = line.as_bytes();
    if start >= bytes.len() || bytes[start] != b'[' {
        return None;
    }
    // Find the matching `]` for the display. Handle escaped `\]` and
    // disallow nested `[` (markdown technically allows them with
    // balanced pairs; we keep it simple and bail on a nested `[`).
    let mut i = start + 1;
    let display_start = i;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if b == b']' {
            break;
        }
        if b == b'[' {
            return None;
        }
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b']' {
        return None;
    }
    let display_end = i;
    // Need `(` immediately after `]`.
    if i + 1 >= bytes.len() || bytes[i + 1] != b'(' {
        return None;
    }
    i += 2;
    let href_start = i;
    let mut depth: i32 = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b')' {
        return None;
    }
    let href_end = i;
    let end = i + 1;

    let raw_href = &line[href_start..href_end];
    let href = strip_md_title(raw_href.trim());
    if href.is_empty() {
        return None;
    }
    if is_external_url(&href) {
        return None;
    }
    let display = line[display_start..display_end].to_string();
    let prefix_offset = if is_embed { 1 } else { 0 };
    let raw_text = line[start - prefix_offset..end].to_string();
    Some((
        end,
        MdLink {
            display,
            href,
            raw_text,
        },
    ))
}

/// Strip the optional `"title"` (or `'title'`, `(title)`) trailing the
/// href in a markdown link, percent-decode the URL, and unwrap the
/// CommonMark angle-bracket form. `<...>` may contain whitespace, so
/// the bracket check has to come before the whitespace split.
fn strip_md_title(href: &str) -> String {
    let trimmed = href.trim();
    // Angle-bracket form `<url>` — URL may legally contain spaces.
    if let Some(after_lt) = trimmed.strip_prefix('<') {
        let end = after_lt.find('>').unwrap_or(after_lt.len());
        let url = &after_lt[..end];
        return urlencoding::decode(url)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| url.to_string());
    }
    // Non-bracket form: title (if any) is whitespace-separated from URL.
    let url = match trimmed.find(|c: char| c.is_whitespace()) {
        Some(idx) => &trimmed[..idx],
        None => trimmed,
    };
    urlencoding::decode(url)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| url.to_string())
}

fn is_external_url(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("obsidian://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("ssh://")
        || lower.starts_with("file://")
}

/// Push a parsed wikilink into `out`. `span` is the file-byte range of
/// the *full* token (including a leading `!` for embeds), but `bytes[i]`
/// at `span.start` is the `!` for embeds, the `[` otherwise.
fn push_wikilink(
    line: &str,
    line_start_offset: usize,
    lineno: usize,
    intra_line_span: std::ops::Range<usize>,
    is_embed: bool,
    out: &mut Vec<RawLink>,
) {
    let raw_text = line[intra_line_span.clone()].to_string();
    // Body is between the inner `[[` and `]]`.
    let body_start_in_token = if is_embed { 3 } else { 2 };
    let body = &raw_text[body_start_in_token..raw_text.len() - 2];
    let (target_text, anchor, display) = split_wiki_body(body);
    if target_text.is_empty() {
        return; // `[[#anchor]]` or `[[|alias]]` with no target — ignore
    }
    let file_span =
        (line_start_offset + intra_line_span.start)..(line_start_offset + intra_line_span.end);
    out.push(RawLink {
        form: LinkForm::WikiLink,
        is_embed,
        byte_range: file_span,
        line: lineno,
        raw_text,
        target_text,
        anchor,
        display,
    });
}

fn push_md_link(
    line_start_offset: usize,
    lineno: usize,
    intra_line_span: std::ops::Range<usize>,
    link: MdLink,
    out: &mut Vec<RawLink>,
) {
    let is_embed = link.raw_text.starts_with('!');
    let (target_text, anchor) = split_anchor(&link.href);
    if target_text.is_empty() {
        return;
    }
    let file_span =
        (line_start_offset + intra_line_span.start)..(line_start_offset + intra_line_span.end);
    out.push(RawLink {
        form: LinkForm::MdLink,
        is_embed,
        byte_range: file_span,
        line: lineno,
        raw_text: link.raw_text,
        target_text,
        anchor,
        display: Some(link.display).filter(|s| !s.is_empty()),
    });
}

/// Split a wikilink body `target[#anchor][|display]` into its three
/// optional pieces. The target text is trimmed; anchor and display
/// preserve internal whitespace.
fn split_wiki_body(body: &str) -> (String, Option<String>, Option<String>) {
    let (lhs, display) = match body.find('|') {
        Some(idx) => (&body[..idx], Some(body[idx + 1..].to_string())),
        None => (body, None),
    };
    let (target, anchor) = match lhs.find('#') {
        Some(idx) => (&lhs[..idx], Some(lhs[idx + 1..].to_string())),
        None => (lhs, None),
    };
    (target.trim().to_string(), anchor, display)
}

/// Split a markdown-link href into `(path_part, anchor)`. The anchor
/// (`#heading`) is preserved separately because target resolution keys
/// off the path part only.
fn split_anchor(href: &str) -> (String, Option<String>) {
    match href.find('#') {
        Some(idx) => (href[..idx].to_string(), Some(href[idx + 1..].to_string())),
        None => (href.to_string(), None),
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    fn extract(s: &str) -> Vec<RawLink> {
        extract_links(s)
    }

    #[test]
    fn empty_input() {
        assert_eq!(extract(""), vec![]);
    }

    #[test]
    fn plain_wikilink() {
        let links = extract("see [[Foo]] now\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].form, LinkForm::WikiLink);
        assert!(!links[0].is_embed);
        assert_eq!(links[0].raw_text, "[[Foo]]");
        assert_eq!(links[0].target_text, "Foo");
        assert_eq!(links[0].anchor, None);
        assert_eq!(links[0].display, None);
        assert_eq!(links[0].line, 1);
    }

    #[test]
    fn wikilink_with_alias() {
        let links = extract("see [[Foo|Bar]]\n");
        assert_eq!(links[0].target_text, "Foo");
        assert_eq!(links[0].display.as_deref(), Some("Bar"));
    }

    #[test]
    fn wikilink_with_anchor() {
        let links = extract("see [[Foo#Heading]]\n");
        assert_eq!(links[0].target_text, "Foo");
        assert_eq!(links[0].anchor.as_deref(), Some("Heading"));
        assert_eq!(links[0].display, None);
    }

    #[test]
    fn wikilink_with_anchor_and_alias() {
        let links = extract("see [[Foo#H|D]]\n");
        assert_eq!(links[0].target_text, "Foo");
        assert_eq!(links[0].anchor.as_deref(), Some("H"));
        assert_eq!(links[0].display.as_deref(), Some("D"));
    }

    #[test]
    fn wikilink_path_form_kept_in_target() {
        let links = extract("see [[Sub/Foo]]\n");
        assert_eq!(links[0].target_text, "Sub/Foo");
    }

    #[test]
    fn embed_wikilink() {
        let links = extract("![[image.png]]\n");
        assert_eq!(links[0].form, LinkForm::WikiLink);
        assert!(links[0].is_embed);
        assert_eq!(links[0].raw_text, "![[image.png]]");
        assert_eq!(links[0].target_text, "image.png");
    }

    #[test]
    fn plain_md_link() {
        let links = extract("see [Foo](foo.md) now\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].form, LinkForm::MdLink);
        assert!(!links[0].is_embed);
        assert_eq!(links[0].raw_text, "[Foo](foo.md)");
        assert_eq!(links[0].target_text, "foo.md");
        assert_eq!(links[0].display.as_deref(), Some("Foo"));
    }

    #[test]
    fn md_link_with_anchor() {
        let links = extract("[F](foo.md#H)\n");
        assert_eq!(links[0].target_text, "foo.md");
        assert_eq!(links[0].anchor.as_deref(), Some("H"));
    }

    #[test]
    fn md_link_url_decoded() {
        let links = extract("[F](My%20Note.md)\n");
        assert_eq!(links[0].target_text, "My Note.md");
    }

    #[test]
    fn md_embed_link() {
        let links = extract("![alt](image.png)\n");
        assert!(links[0].is_embed);
        assert_eq!(links[0].target_text, "image.png");
    }

    #[test]
    fn md_link_external_url_not_an_edge() {
        let links = extract("[google](https://google.com) and [m](mailto:a@b)\n");
        assert!(links.is_empty());
    }

    #[test]
    fn skips_links_inside_fenced_code_block() {
        let s = "\
before [[A]]
```
[[B]]
[c](c.md)
```
after [[D]]
";
        let links = extract(s);
        let targets: Vec<_> = links.iter().map(|l| l.target_text.as_str()).collect();
        assert_eq!(targets, vec!["A", "D"]);
    }

    #[test]
    fn skips_links_inside_indented_code_block() {
        let s = "before [[A]]\n    [[B]]\nafter [[C]]\n";
        let links = extract(s);
        let targets: Vec<_> = links.iter().map(|l| l.target_text.as_str()).collect();
        assert_eq!(targets, vec!["A", "C"]);
    }

    #[test]
    fn skips_links_inside_frontmatter() {
        let s = "---\ntitle: [[Foo]]\n---\n[[Real]]\n";
        let links = extract(s);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "Real");
    }

    #[test]
    fn skips_links_inside_inline_code_span() {
        let links = extract("text `[[Foo]]` more [[Bar]]\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "Bar");
    }

    #[test]
    fn unterminated_code_span_does_not_swallow_following_links() {
        // CommonMark: an unterminated `` ` `` is literal — links on the
        // rest of the line still register.
        let links = extract("`unterminated [[Foo]]\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "Foo");
    }

    #[test]
    fn double_backtick_code_span_skips_inner_single_backticks() {
        let links = extract("``[[Foo]] `still in code` [[Bar]]`` real [[Baz]]\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "Baz");
    }

    #[test]
    fn byte_range_round_trips_for_each_link() {
        let s = "line one [[A]] mid\nline two [B](b.md) end\n";
        let links = extract(s);
        assert_eq!(links.len(), 2);
        for l in &links {
            assert_eq!(&s[l.byte_range.clone()], l.raw_text);
        }
    }

    #[test]
    fn line_numbers_track_real_source_lines() {
        let s = "first\n[[A]]\n\n[[B]]\n";
        let links = extract(s);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].line, 2);
        assert_eq!(links[1].line, 4);
    }

    #[test]
    fn multiple_links_on_one_line_in_document_order() {
        let s = "[[A]] then [[B]] then [c](c.md)\n";
        let links = extract(s);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target_text, "A");
        assert_eq!(links[1].target_text, "B");
        assert_eq!(links[2].target_text, "c.md");
    }

    #[test]
    fn duplicate_links_emit_one_record_each() {
        let s = "[[Foo]] [[Foo]] [[Foo]]\n";
        let links = extract(s);
        assert_eq!(links.len(), 3);
        for l in &links {
            assert_eq!(l.target_text, "Foo");
        }
    }

    #[test]
    fn empty_wikilink_body_is_not_a_link() {
        let links = extract("[[]] is not a link\n");
        assert!(links.is_empty());
    }

    #[test]
    fn empty_target_with_anchor_is_not_a_link() {
        // `[[#Heading]]` is Obsidian shorthand for "this file's heading"
        // — not a cross-note edge. We don't emit it.
        let links = extract("[[#Heading]]\n");
        assert!(links.is_empty());
    }

    #[test]
    fn extension_less_md_link_kept_verbatim() {
        let links = extract("[F](notes/foo)\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "notes/foo");
    }

    #[test]
    fn md_link_with_title_strips_title() {
        let links = extract(r#"[F](foo.md "the title")"#);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "foo.md");
    }

    #[test]
    fn md_link_angle_bracket_form() {
        let links = extract("[F](<foo bar.md>)\n");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_text, "foo bar.md");
    }
}
