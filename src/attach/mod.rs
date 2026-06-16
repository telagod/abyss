//! `abyss attach <host>` — install agent-side hooks idempotently.
//!
//! Each supported host has its own settings layout; the shared contract is
//! that re-running `attach` against an already-configured host must be a
//! no-op (no duplicate hook entries, no clobbered unrelated config).

pub mod claude;
