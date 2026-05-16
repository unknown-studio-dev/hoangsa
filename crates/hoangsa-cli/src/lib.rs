//! Library facade exposing the CLI's command modules so other crates
//! (notably `hoangsa-ui-server`) can reuse the rule, addon, and config
//! logic without shelling out to the binary. The CLI binary at `main.rs`
//! is unchanged — both targets share the same source tree.

pub mod cmd;
pub mod helpers;
