//! The archive formats and the operations over them.
//!
//! Everything here reads and writes plain files. No module in `core` holds
//! state between calls, and the SQLite index in `index` is the only
//! derived artefact that is not itself human-readable — it can be deleted
//! at any time and rebuilt from the Markdown files.

pub mod clock;
pub mod crypto;
pub mod dates;
pub mod env;
pub mod index;
pub mod interview;
pub mod llm;
pub mod manifest;
pub mod media;
pub mod readme;
pub mod seal;
pub mod story;
pub mod timeline;
pub mod vault;
pub mod voice;
pub mod yaml;
