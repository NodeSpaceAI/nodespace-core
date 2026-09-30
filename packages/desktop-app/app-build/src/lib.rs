//! Build-script helpers for an app crate built on core's Tauri library.
//!
//! `tauri_build::build()` copies every sidecar (`bundle.externalBin`) and every
//! resource (`bundle.resources`) that the app's `tauri.conf.json` declares into
//! the build output, and fails on one that is missing. Both are usually build
//! output, and a fresh checkout has neither. The two helpers here keep that
//! from getting in the way, and both are called from the app crate's
//! `build.rs` before `tauri_build::build()`:
//!
//! - [`drop_unstaged_bundle_entries`] lets a debug build, and the tests that
//!   compile the app crate, run without staged sidecars and resources. A
//!   release build stays strict.
//! - [`sync_stale_sidecar`] stops `tauri_build` from overwriting a fresh
//!   `target/<profile>/<bin>` with a stale staged sidecar.
//!
//! ```ignore
//! fn main() {
//!     // Both run before `tauri_build::build()`, whose unconditional copy of
//!     // each sidecar is what `sync_stale_sidecar` guards against.
//!     if let Err(err) = nodespace_app_build::sync_stale_sidecar(&target_bin, &sidecar_bin) {
//!         println!("cargo:warning=could not reconcile sidecar staging: {err}");
//!     }
//!     nodespace_app_build::drop_unstaged_bundle_entries();
//!     tauri_build::build()
//! }
//! ```
//!
//! The crate has no `tauri` dependency, so it compiles and tests on every
//! platform, and an app crate can take it as a build dependency.

mod bundle_entries;
mod sidecar_staging;

pub use bundle_entries::drop_unstaged_bundle_entries;
pub use sidecar_staging::sync_stale_sidecar;
