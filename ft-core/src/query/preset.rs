//! Built-in query presets.
//!
//! User-defined presets in [`Config::presets`](crate::config::Config::presets)
//! shadow built-ins of the same name. Resolution lives in the CLI; this module
//! just owns the canonical built-in definitions as DSL strings so they round-
//! trip through the same parser as user queries.

/// Return the DSL string for a built-in preset, or `None` if unknown.
pub fn builtin(name: &str) -> Option<&'static str> {
    Some(match name {
        "today" => "not done and (due on today or scheduled on today)",
        "overdue" => "not done and due before today",
        "upcoming" => "not done and due after today",
        "done-today" => "done and completed on today",
        _ => return None,
    })
}

/// Names of all built-in presets, sorted, for help text and shell completions.
pub fn builtin_names() -> &'static [&'static str] {
    &["done-today", "overdue", "today", "upcoming"]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::dsl;
    use chrono::NaiveDate;

    #[test]
    fn every_builtin_parses() {
        let today = NaiveDate::from_ymd_opt(2026, 5, 9).unwrap();
        for name in builtin_names() {
            let dsl_str = builtin(name).unwrap_or_else(|| panic!("missing preset {name}"));
            dsl::parse(dsl_str, today)
                .unwrap_or_else(|e| panic!("preset `{name}` failed to parse: {e}"));
        }
    }

    #[test]
    fn unknown_preset_returns_none() {
        assert!(builtin("nope").is_none());
    }
}
