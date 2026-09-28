//! Golden-prompt tuning: prompts as data, run against the real inference path
//! with no NodeSpace plumbing in between.
//!
//! The deliverable is the **case files** — a set of prompt strings that
//! reliably get the right tool call, which the real assembly pipeline is then
//! engineered to reproduce. The code here is the scaffolding that produces
//! them: read a case file, call the model through production's own inference
//! path, print what came back. The human reads the output and decides.
//!
//! There is deliberately no assertion, scoring, or pass/fail. The tuning loop
//! is *look at the response and judge it*, and encoding an expected answer
//! would only pay off if something automated consumed it. Nothing here does —
//! holding production to a frozen golden is a separate concern, and end-to-end
//! scoring is the agent matrix's job. A third scoring system would duplicate
//! both, and would be new surface for the failure mode this project has
//! already paid for: a harness bug reported as a model failure.
//!
//! The methodology comes from `tests/it/golden_scenario6_handauthored.rs` and
//! `tests/it/golden_scenario6_sequence.rs`, which stay in place as the validated
//! Rust-side reference. They proved the approach but stayed bespoke — one
//! scenario, prompts as string literals, a recompile per edit. Here the prompt
//! is a TOML file and the loop is edit-and-rerun.
//!
//! Two questions this keeps apart:
//!
//! - *Does this exact prompt text get the right tool call?* — seconds, here.
//! - *Does the real pipeline assemble that exact prompt?* — deterministic and
//!   model-free, and not this module's job.
//!
//! Conflating them is what made every prior checkpoint cost a daemon rebuild,
//! a database purge, and minutes per iteration.
//!
//! [`case`] owns the file format and its loading, both model-free and
//! unit-tested in the default run. [`runner`] owns execution and is reachable
//! only through the `golden_runner` bin, which is outside `cargo test`
//! entirely — it loads a ~5GB GGUF.
//!
//! Committed cases live in `packages/agent/goldens/`.

pub mod case;
pub mod runner;
