//! Shared TUI widgets used across multiple tabs / views.

pub mod edit_buffer;
pub mod picker;

pub use edit_buffer::EditBuffer;
// Re-exported eagerly so the picker is reachable as
// `crate::tui::widgets::FuzzyPicker` once plan-004 session 4 wires it in.
// `#[allow(unused_imports)]` keeps the re-exports legal until then.
#[allow(unused_imports)]
pub use picker::{FuzzyPicker, PickerItem, PickerOutcome, PickerSource, VaultFilePickerSource};
