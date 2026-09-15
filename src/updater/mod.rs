//! Update checking, verification, replacement, and restart.

pub mod apply;
pub mod check;
pub mod error;

pub use check::{check_for_update, UpdateInfo};
pub use error::UpdateError;
