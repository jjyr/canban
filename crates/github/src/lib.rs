//! GitHub protocol and credentials. No Card, database, UI or execution policy.
pub mod auth;
mod client;
mod credentials;
pub mod models;
pub use client::{Client, remote_repository, repository, segment};
