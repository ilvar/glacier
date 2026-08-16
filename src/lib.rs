//! `legacy` — a local-first life-story vault and digital replica.
//!
//! The library is organised so the archive is the product and the code is
//! replaceable: `core` owns the on-disk formats, `cap` owns every effect on
//! the outside world, and `cli`/`api` are two thin front ends over the same
//! functions. The replica in `core::llm` is a reader of the archive, not a
//! component of it, and can be deleted without affecting anything else.

#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

pub mod api;
pub mod args;
pub mod cap;
pub mod cli;
pub mod core;
pub mod tui;
