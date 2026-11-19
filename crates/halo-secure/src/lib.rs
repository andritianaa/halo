//! Halo security modules: encrypted vault (M10), per-process network monitor/limiter (M5).
//!
//! Both are opt-in (off until the user enables them). Nothing here changes system
//! settings, firewall rules, drivers or the registry.

pub mod net;
pub mod traffic;
pub mod vault;
mod worker;

use halo_core::config::Config;
use halo_core::module::Registry;

/// Typed handles the UI uses to read module state / trigger actions.
#[derive(Clone)]
pub struct Handles {
    pub vault: vault::VaultHandle,
    pub net: net::NetHandle,
}

/// Build the security modules and register them.
pub fn register(registry: &mut Registry, _config: &Config) -> Handles {
    let vault = vault::Vault::new();
    let net = net::Net::new();
    let handles = Handles { vault: vault.handle(), net: net.handle() };
    registry.register(Box::new(vault));
    registry.register(Box::new(net));
    handles
}
