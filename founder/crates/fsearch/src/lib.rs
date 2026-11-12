//! Whole-disk file search for macOS: a fuzzy name index kept live from FSEvents, plus a trigram content index.

pub mod content;
mod engine;
mod fsevents;
pub mod index;
pub mod live;
pub mod query;
pub mod walk;

pub use content::{FileMatches, Grep, GrepResult};
pub use engine::{Engine, Found, Options, Status, default_dir, gated, has_full_disk_access, no_materialize};
pub use live::Totals;
pub use query::{GrepMode, Query};
