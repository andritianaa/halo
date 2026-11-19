//! `halo-ui` — shared design tokens, motion and GPUI/gpui-kit glue.
//!
//! Every visual value used by Halo comes from [`tokens`]. See `design/DESIGN-SYSTEM.md`
//! and the `halo-design-system` skill.

pub mod mark;
pub mod motion;
pub mod tokens;

#[cfg(feature = "gpui")]
pub mod gpui_glue;

#[cfg(feature = "gpui")]
pub use gpui_glue::*;
