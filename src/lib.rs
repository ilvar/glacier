//! `legacy` — a local-first life-story vault and digital replica.

#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

pub mod cap;
pub mod core;
