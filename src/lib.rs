//! `co-review` — interactive, split-screen PR co-review between you and your AI
//! agent, inside [Herdr](https://herdr.dev).
//!
//! See `docs/DECISIONS.md` for the design rationale. The crate is split into:
//!
//! - [`model`] — the shared `State` and everything in it.
//! - [`store`] — lock-guarded persistence of that state.
//! - [`util`] — small shared helpers.

pub mod model;
pub mod store;
pub mod util;
