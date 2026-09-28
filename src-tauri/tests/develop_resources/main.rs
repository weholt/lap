//! Integration tests for content-addressed develop resources (lap-62b /
//! TASK-405; governing contract `docs/raw-development/spec.md`, acceptance
//! A7/A8/A9 and the schema's resource rules).
//!
//! Mirrors `tests/develop_assets`: real files on disk, real sidecars through
//! `RecipeRepository`, and the real copy-fork primitives from
//! `asset_operations`.

mod common;
mod lens_cases;
mod portability_cases;
mod store_cases;
