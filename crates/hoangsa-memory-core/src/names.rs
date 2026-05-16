//! Canonical filenames for memory surface files. Centralized here so a
//! rename only touches one place — the policy crate, store crate, and any
//! external consumer all import these constants.

/// User preferences file at the root of a memory directory.
pub const USER_MD: &str = "USER.md";

/// Per-project facts file.
pub const MEMORY_MD: &str = "MEMORY.md";

/// Action-triggered advice file.
pub const LESSONS_MD: &str = "LESSONS.md";
