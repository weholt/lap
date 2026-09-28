//! Develop rollback-switch integration tests (lap-63f / TASK-602,
//! managed continuation of lap-404.2). Governing contract:
//! docs/raw-development/spec.md ("Persistence and compatibility").

mod common;
mod entry_gate;
mod flag_store;
mod migration_scenarios;
mod non_destructive;
