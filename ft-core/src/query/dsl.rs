//! Hand-rolled tokenizer + recursive-descent parser for the supported subset
//! of the Obsidian Tasks query language.
//!
//! Grammar:
//! ```text
//! query     = or_expr [ "sort" "by" sort_keys ] [ "limit" integer ]
//! or_expr   = and_expr ( "or" and_expr )*
//! and_expr  = unary  ( "and" unary  )*
//! unary     = "not" unary | atom
//! atom      = "(" or_expr ")" | predicate
//!
//! predicate = "status" "is" status_val
//!           | "priority" "is" prio_val
//!           | "path" "includes" string
//!           | ( "tag" "is" | "has" "tag" ) tag_val
//!           | "due"       ("before"|"after"|"on") date_val
//!           | "scheduled" ("before"|"after"|"on") date_val
//!           | "completed" ("before"|"after"|"on") date_val
//!           | "done"
//!           | "has" "due" [ "date" ]
//!           | "no" "due" "date"
//!
//! sort_keys = sort_key ( "," sort_key )*
//! sort_key  = ("due"|"scheduled"|"priority"|"path"|"description"|"status")
//!             [ "reverse" ]
//!
//! date_val  = YYYY-MM-DD | "today" | "tomorrow" | "yesterday"
//! ```
//!
//! Anything outside this grammar is rejected with [`DslError`] pointing at the
//! exact offending token.

use std::fmt;

use chrono::{Duration, NaiveDate};

use crate::task::{Priority, Status};

use super::expr::{Atom, Expr};
use super::sort::{SortKey, SortOrder};

/// A compiled DSL query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub expr: Option<Expr>,
    pub sort_keys: Vec<(SortKey, SortOrder)>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DslError {
    UnexpectedToken { found: String, expected: String },
    UnknownIdentifier(String),
    InvalidDate(String),
    InvalidNumber(String),
    UnterminatedString,
    UnsupportedFeature(String),
    EmptyInput,
    TrailingTokens(String),
}

impl fmt::Display for DslError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DslError::UnexpectedToken { found, expected } => {
                write!(f, "expected {expected}, found `{found}`")
            }
            DslError::UnknownIdentifier(s) => write!(f, "unknown identifier `{s}`"),
            DslError::InvalidDate(s) => {
                write!(
                    f,
                    "invalid date `{s}` (expected YYYY-MM-DD or today/tomorrow/yesterday)"
                )
            }
            DslError::InvalidNumber(s) => write!(f, "invalid number `{s}`"),
            DslError::UnterminatedString => write!(f, "unterminated string literal"),
            DslError::UnsupportedFeature(s) => write!(
                f,
                "unsupported query feature: `{s}` — see docs/query-dsl.md"
            ),
            DslError::EmptyInput => write!(f, "empty query"),
            DslError::TrailingTokens(s) => {
                write!(f, "unexpected trailing tokens after query: `{s}`")
            }
        }
    }
}

impl std::error::Error for DslError {}

pub type DslResult<T> = std::result::Result<T, DslError>;

/// Compile a DSL query string into an [`Query`].
///
/// `today` is injected so callers can pin "today"/"tomorrow"/"yesterday" to a
/// specific date in tests.
pub fn parse(input: &str, today: NaiveDate) -> DslResult<Query> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(DslError::EmptyInput);
    }
    let mut p = Parser {
        tokens,
        pos: 0,
        today,
    };
    let expr = p.parse_or()?;
    let sort_keys = if p.peek_kw("sort") {
        p.advance();
        p.expect_kw("by")?;
        p.parse_sort_keys()?
    } else {
        Vec::new()
    };
    let limit = if p.peek_kw("limit") {
        p.advance();
        Some(p.parse_number()?)
    } else {
        None
    };
    if p.pos < p.tokens.len() {
        let rest: Vec<&str> = p.tokens[p.pos..].iter().map(|t| t.as_str()).collect();
        return Err(DslError::TrailingTokens(rest.join(" ")));
    }
    Ok(Query {
        expr: Some(expr),
        sort_keys,
        limit,
    })
}

// ── tokenizer ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Token {
    Word(String),
    /// Quoted string literal; `as_str()` returns the unquoted content.
    QuotedString(String),
    LParen,
    RParen,
    Comma,
}

impl Token {
    fn as_str(&self) -> &str {
        match self {
            Token::Word(s) | Token::QuotedString(s) => s.as_str(),
            Token::LParen => "(",
            Token::RParen => ")",
            Token::Comma => ",",
        }
    }
}

fn tokenize(input: &str) -> DslResult<Vec<Token>> {
    let mut out = Vec::new();
    let mut iter = input.chars().peekable();
    while let Some(&c) = iter.peek() {
        match c {
            ws if ws.is_whitespace() => {
                iter.next();
            }
            '(' => {
                iter.next();
                out.push(Token::LParen);
            }
            ')' => {
                iter.next();
                out.push(Token::RParen);
            }
            ',' => {
                iter.next();
                out.push(Token::Comma);
            }
            '"' => {
                iter.next();
                let mut buf = String::new();
                let mut closed = false;
                for ch in iter.by_ref() {
                    if ch == '"' {
                        closed = true;
                        break;
                    }
                    buf.push(ch);
                }
                if !closed {
                    return Err(DslError::UnterminatedString);
                }
                out.push(Token::QuotedString(buf));
            }
            _ => {
                let mut buf = String::new();
                while let Some(&ch) = iter.peek() {
                    if ch.is_whitespace() || matches!(ch, '(' | ')' | ',' | '"') {
                        break;
                    }
                    buf.push(ch);
                    iter.next();
                }
                out.push(Token::Word(buf));
            }
        }
    }
    Ok(out)
}

// ── parser ───────────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    today: NaiveDate,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn peek_word(&self) -> Option<&str> {
        match self.peek() {
            Some(Token::Word(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    fn peek_kw(&self, kw: &str) -> bool {
        matches!(self.peek_word(), Some(w) if w.eq_ignore_ascii_case(kw))
    }

    fn expect_kw(&mut self, kw: &str) -> DslResult<()> {
        match self.peek() {
            Some(Token::Word(w)) if w.eq_ignore_ascii_case(kw) => {
                self.pos += 1;
                Ok(())
            }
            other => Err(DslError::UnexpectedToken {
                found: other.map(|t| t.as_str().to_string()).unwrap_or_default(),
                expected: format!("`{kw}`"),
            }),
        }
    }

    fn parse_or(&mut self) -> DslResult<Expr> {
        let mut parts = vec![self.parse_and()?];
        while self.peek_kw("or") {
            self.advance();
            parts.push(self.parse_and()?);
        }
        Ok(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            Expr::Or(parts)
        })
    }

    fn parse_and(&mut self) -> DslResult<Expr> {
        let mut parts = vec![self.parse_unary()?];
        while self.peek_kw("and") {
            self.advance();
            parts.push(self.parse_unary()?);
        }
        Ok(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            Expr::And(parts)
        })
    }

    fn parse_unary(&mut self) -> DslResult<Expr> {
        // `not done` is a primitive, but `not (something)` is negation.
        // We only treat `not` as negation if it's NOT followed by `done` —
        // otherwise we let the predicate parser handle `not done` so the AST
        // stores it as `Atom::NotDone` (cleaner snapshots, identical behavior).
        if self.peek_kw("not") {
            // Look ahead one token
            let next = self.tokens.get(self.pos + 1);
            let is_not_done =
                matches!(next, Some(Token::Word(w)) if w.eq_ignore_ascii_case("done"));
            if !is_not_done {
                self.advance();
                let inner = self.parse_unary()?;
                return Ok(Expr::Not(Box::new(inner)));
            }
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> DslResult<Expr> {
        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance();
            let inner = self.parse_or()?;
            match self.peek() {
                Some(Token::RParen) => {
                    self.advance();
                    Ok(inner)
                }
                other => Err(DslError::UnexpectedToken {
                    found: other.map(|t| t.as_str().to_string()).unwrap_or_default(),
                    expected: "`)`".into(),
                }),
            }
        } else {
            self.parse_predicate()
        }
    }

    fn parse_predicate(&mut self) -> DslResult<Expr> {
        let head = self.peek_word().ok_or_else(|| DslError::UnexpectedToken {
            found: self
                .peek()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            expected: "predicate".into(),
        })?;
        let head = head.to_ascii_lowercase();

        match head.as_str() {
            "status" => {
                self.advance();
                self.expect_kw("is")?;
                let v = self.parse_string_value()?;
                Ok(Expr::Atom(Atom::Status(parse_status(&v)?)))
            }
            "priority" => {
                self.advance();
                self.expect_kw("is")?;
                let v = self.parse_string_value()?;
                Ok(Expr::Atom(Atom::Priority(parse_priority(&v)?)))
            }
            "path" => {
                self.advance();
                self.expect_kw("includes")?;
                let v = self.parse_string_value()?;
                Ok(Expr::Atom(Atom::PathIncludes(v)))
            }
            "tag" => {
                self.advance();
                self.expect_kw("is")?;
                let v = self.parse_string_value()?;
                Ok(Expr::Atom(Atom::HasTag(strip_hash(&v))))
            }
            "has" => {
                self.advance();
                let next = self
                    .peek_word()
                    .ok_or_else(|| DslError::UnexpectedToken {
                        found: self
                            .peek()
                            .map(|t| t.as_str().to_string())
                            .unwrap_or_default(),
                        expected: "`tag` or `due`".into(),
                    })?
                    .to_ascii_lowercase();
                match next.as_str() {
                    "tag" => {
                        self.advance();
                        let v = self.parse_string_value()?;
                        Ok(Expr::Atom(Atom::HasTag(strip_hash(&v))))
                    }
                    "due" => {
                        self.advance();
                        // optional trailing `date`
                        if self.peek_kw("date") {
                            self.advance();
                        }
                        Ok(Expr::Atom(Atom::HasDue))
                    }
                    other => Err(DslError::UnexpectedToken {
                        found: other.into(),
                        expected: "`tag` or `due`".into(),
                    }),
                }
            }
            "no" => {
                self.advance();
                self.expect_kw("due")?;
                self.expect_kw("date")?;
                Ok(Expr::Atom(Atom::NoDue))
            }
            "due" => {
                self.advance();
                let cmp = self.parse_cmp()?;
                let date = self.parse_date_value()?;
                Ok(Expr::Atom(match cmp {
                    Cmp::Before => Atom::DueBefore(date),
                    Cmp::After => Atom::DueAfter(date),
                    Cmp::On => Atom::DueOn(date),
                }))
            }
            "scheduled" => {
                self.advance();
                let cmp = self.parse_cmp()?;
                let date = self.parse_date_value()?;
                Ok(Expr::Atom(match cmp {
                    Cmp::Before => Atom::ScheduledBefore(date),
                    Cmp::After => Atom::ScheduledAfter(date),
                    Cmp::On => Atom::ScheduledOn(date),
                }))
            }
            "completed" => {
                self.advance();
                let cmp = self.parse_cmp()?;
                let date = self.parse_date_value()?;
                Ok(Expr::Atom(match cmp {
                    Cmp::Before => Atom::CompletedBefore(date),
                    Cmp::After => Atom::CompletedAfter(date),
                    Cmp::On => Atom::CompletedOn(date),
                }))
            }
            "done" => {
                self.advance();
                Ok(Expr::Atom(Atom::Done))
            }
            "not" => {
                self.advance();
                self.expect_kw("done")?;
                Ok(Expr::Atom(Atom::NotDone))
            }
            // Reject features that the plugin supports but we don't.
            "filter" | "group" | "hide" | "show" | "short" | "explain" | "ignore" => {
                Err(DslError::UnsupportedFeature(head))
            }
            other => Err(DslError::UnknownIdentifier(other.into())),
        }
    }

    fn parse_cmp(&mut self) -> DslResult<Cmp> {
        let w = self.peek_word().ok_or_else(|| DslError::UnexpectedToken {
            found: self
                .peek()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            expected: "`before`, `after`, or `on`".into(),
        })?;
        let cmp = match w.to_ascii_lowercase().as_str() {
            "before" => Cmp::Before,
            "after" => Cmp::After,
            "on" => Cmp::On,
            other => {
                return Err(DslError::UnexpectedToken {
                    found: other.into(),
                    expected: "`before`, `after`, or `on`".into(),
                })
            }
        };
        self.advance();
        Ok(cmp)
    }

    fn parse_string_value(&mut self) -> DslResult<String> {
        match self.advance() {
            Some(Token::Word(s)) | Some(Token::QuotedString(s)) => Ok(s.clone()),
            other => Err(DslError::UnexpectedToken {
                found: other.map(|t| t.as_str().to_string()).unwrap_or_default(),
                expected: "value".into(),
            }),
        }
    }

    fn parse_date_value(&mut self) -> DslResult<NaiveDate> {
        let s = self.parse_string_value()?;
        match s.to_ascii_lowercase().as_str() {
            "today" => Ok(self.today),
            "tomorrow" => Ok(self.today + Duration::days(1)),
            "yesterday" => Ok(self.today - Duration::days(1)),
            _ => NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|_| DslError::InvalidDate(s)),
        }
    }

    fn parse_number(&mut self) -> DslResult<usize> {
        let s = self.parse_string_value()?;
        s.parse::<usize>().map_err(|_| DslError::InvalidNumber(s))
    }

    fn parse_sort_keys(&mut self) -> DslResult<Vec<(SortKey, SortOrder)>> {
        let mut keys = Vec::new();
        keys.push(self.parse_one_sort_key()?);
        while matches!(self.peek(), Some(Token::Comma)) {
            self.advance();
            keys.push(self.parse_one_sort_key()?);
        }
        Ok(keys)
    }

    fn parse_one_sort_key(&mut self) -> DslResult<(SortKey, SortOrder)> {
        let s = self.parse_string_value()?;
        let key = parse_sort_key(&s)?;
        let order = if self.peek_kw("reverse") {
            self.advance();
            SortOrder::Desc
        } else {
            SortOrder::Asc
        };
        Ok((key, order))
    }
}

enum Cmp {
    Before,
    After,
    On,
}

fn strip_hash(s: &str) -> String {
    s.trim_start_matches('#').to_string()
}

fn parse_status(s: &str) -> DslResult<Status> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "open" | "todo" => Status::Open,
        "done" | "complete" | "completed" => Status::Done,
        "in-progress" | "in_progress" | "doing" => Status::InProgress,
        "cancelled" | "canceled" => Status::Cancelled,
        other => return Err(DslError::UnknownIdentifier(format!("status `{other}`"))),
    })
}

fn parse_priority(s: &str) -> DslResult<Priority> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "highest" => Priority::Highest,
        "high" => Priority::High,
        "medium" | "normal" => Priority::Medium,
        "low" => Priority::Low,
        "lowest" => Priority::Lowest,
        other => return Err(DslError::UnknownIdentifier(format!("priority `{other}`"))),
    })
}

pub fn parse_sort_key(s: &str) -> DslResult<SortKey> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "due" => SortKey::Due,
        "scheduled" => SortKey::Scheduled,
        "priority" => SortKey::Priority,
        "path" => SortKey::Path,
        "description" => SortKey::Description,
        "status" => SortKey::Status,
        other => return Err(DslError::UnknownIdentifier(format!("sort key `{other}`"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 9).unwrap()
    }

    fn parse_ok(s: &str) -> Query {
        parse(s, today()).expect("DSL should parse")
    }

    #[test]
    fn empty_input_errors() {
        assert!(matches!(parse("", today()), Err(DslError::EmptyInput)));
        assert!(matches!(parse("   ", today()), Err(DslError::EmptyInput)));
    }

    #[test]
    fn status_predicate() {
        let q = parse_ok("status is open");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::Status(Status::Open))));
        let q = parse_ok("status is in-progress");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::Status(Status::InProgress))));
    }

    #[test]
    fn priority_predicate() {
        let q = parse_ok("priority is high");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::Priority(Priority::High))));
    }

    #[test]
    fn path_includes_quoted() {
        let q = parse_ok(r#"path includes "Projects/""#);
        assert_eq!(
            q.expr,
            Some(Expr::Atom(Atom::PathIncludes("Projects/".into())))
        );
    }

    #[test]
    fn tag_predicates_strip_hash() {
        let q = parse_ok("tag is #work");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::HasTag("work".into()))));
        let q = parse_ok("has tag work");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::HasTag("work".into()))));
    }

    #[test]
    fn due_comparisons() {
        let q = parse_ok("due before 2026-05-15");
        assert_eq!(
            q.expr,
            Some(Expr::Atom(Atom::DueBefore(
                NaiveDate::from_ymd_opt(2026, 5, 15).unwrap()
            )))
        );
        let q = parse_ok("due on today");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::DueOn(today()))));
        let q = parse_ok("due after yesterday");
        assert_eq!(
            q.expr,
            Some(Expr::Atom(Atom::DueAfter(
                NaiveDate::from_ymd_opt(2026, 5, 8).unwrap()
            )))
        );
    }

    #[test]
    fn scheduled_and_completed() {
        let q = parse_ok("scheduled on tomorrow");
        assert_eq!(
            q.expr,
            Some(Expr::Atom(Atom::ScheduledOn(
                NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()
            )))
        );
        let q = parse_ok("completed on today");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::CompletedOn(today()))));
    }

    #[test]
    fn done_and_not_done() {
        let q = parse_ok("done");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::Done)));
        let q = parse_ok("not done");
        assert_eq!(q.expr, Some(Expr::Atom(Atom::NotDone)));
    }

    #[test]
    fn has_due_no_due() {
        assert_eq!(parse_ok("has due").expr, Some(Expr::Atom(Atom::HasDue)));
        assert_eq!(
            parse_ok("has due date").expr,
            Some(Expr::Atom(Atom::HasDue))
        );
        assert_eq!(parse_ok("no due date").expr, Some(Expr::Atom(Atom::NoDue)));
    }

    #[test]
    fn boolean_combinators() {
        let q = parse_ok("status is open and priority is high");
        assert!(matches!(q.expr, Some(Expr::And(_))));

        let q = parse_ok("done or not done");
        assert!(matches!(q.expr, Some(Expr::Or(_))));

        let q = parse_ok("not (done)");
        assert!(matches!(q.expr, Some(Expr::Not(_))));
    }

    #[test]
    fn precedence_and_binds_tighter_than_or() {
        // a or b and c → a or (b and c)
        let q = parse_ok("done or status is open and priority is high");
        match q.expr {
            Some(Expr::Or(parts)) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[1], Expr::And(_)));
            }
            other => panic!("expected Or at the top, got {other:?}"),
        }
    }

    #[test]
    fn parens_override_precedence() {
        let q = parse_ok("(done or not done) and priority is high");
        assert!(matches!(q.expr, Some(Expr::And(_))));
    }

    #[test]
    fn sort_clause() {
        let q = parse_ok("done sort by priority");
        assert_eq!(q.sort_keys, vec![(SortKey::Priority, SortOrder::Asc)]);

        let q = parse_ok("done sort by priority reverse, due");
        assert_eq!(
            q.sort_keys,
            vec![
                (SortKey::Priority, SortOrder::Desc),
                (SortKey::Due, SortOrder::Asc),
            ]
        );
    }

    #[test]
    fn limit_clause() {
        let q = parse_ok("done limit 5");
        assert_eq!(q.limit, Some(5));
    }

    #[test]
    fn sort_then_limit() {
        let q = parse_ok("done sort by due limit 3");
        assert_eq!(q.sort_keys, vec![(SortKey::Due, SortOrder::Asc)]);
        assert_eq!(q.limit, Some(3));
    }

    #[test]
    fn unsupported_feature_rejected() {
        let err = parse("group by path", today()).unwrap_err();
        assert!(matches!(err, DslError::UnsupportedFeature(_)));
    }

    #[test]
    fn unknown_identifier_rejected() {
        let err = parse("foo is bar", today()).unwrap_err();
        assert!(matches!(err, DslError::UnknownIdentifier(_)));
    }

    #[test]
    fn invalid_date_rejected() {
        let err = parse("due on not-a-date", today()).unwrap_err();
        assert!(matches!(err, DslError::InvalidDate(_)));
    }

    #[test]
    fn missing_keyword_rejected() {
        let err = parse("status open", today()).unwrap_err();
        assert!(matches!(err, DslError::UnexpectedToken { .. }));
    }

    #[test]
    fn unterminated_string_rejected() {
        let err = parse(r#"path includes "no-closing-quote"#, today()).unwrap_err();
        assert!(matches!(err, DslError::UnterminatedString));
    }

    #[test]
    fn trailing_tokens_rejected() {
        let err = parse("done extra", today()).unwrap_err();
        assert!(matches!(err, DslError::TrailingTokens(_)));
    }

    #[test]
    fn case_insensitive_keywords() {
        let q = parse_ok("DONE AND PRIORITY IS HIGH");
        assert!(matches!(q.expr, Some(Expr::And(_))));
    }

    #[test]
    fn parse_sort_key_helpers() {
        assert_eq!(parse_sort_key("Due").unwrap(), SortKey::Due);
        assert!(parse_sort_key("nonsense").is_err());
    }
}
