//! `halo-core` — lifecycle-independent building blocks of Halo.
//!
//! - [`config`]: `%APPDATA%\Halo\config.toml`, typed and versioned.
//! - [`module`]: the common `Module` trait + registry with crash isolation.
//! - [`bus`]: message bus between core and modules (no shared internals).
//! - [`overlay`]: anchor geometry, orientation, auto-hide state machine.
//! - [`i18n`]: FR/EN string table.

pub mod bus;
pub mod config;
pub mod dropipc;
pub mod i18n;
pub mod module;
pub mod overlay;
