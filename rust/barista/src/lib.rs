//! Barista for Windows and Linux: shared sessions, local control, power requests, agent hooks,
//! and the MCP server. Mirrors the Swift `BaristaCore` so every platform speaks the same protocol.
pub mod active_ping;
pub mod activity;
pub mod claude_account;
pub mod codex_account;
pub mod connection;
pub mod control;
pub mod hooks;
pub mod limit_alerts;
pub mod mcp;
pub mod models;
pub mod power;
pub mod session;
pub mod system;
pub mod update;
pub mod usage;

pub use models::*;
