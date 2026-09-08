//! Web UI crate public API

pub mod server;

pub use server::{WebUiState, create_router, serve};