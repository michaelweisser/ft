//! Programmatic query API: filter, sort, and DSL-compiled expressions.
//!
//! Two complementary entry points:
//!
//! * [`Filter`] — flag-based, conjunctive, used by CLI flags directly.
//! * [`Expr`] — boolean-tree, compiled from DSL strings via [`dsl::parse`].
//!
//! `tasks list` applies them in sequence (filter → expr → sort → limit), which
//! is semantically equivalent to and-composing them.

pub mod dsl;
pub mod expr;
pub mod filter;
pub mod preset;
pub mod sort;

pub use dsl::{parse as parse_dsl, DslError, Query};
pub use expr::{Atom, Expr};
pub use filter::Filter;
pub use sort::{default_sort, sort_by_keys, SortKey, SortOrder};
