//! # aldrava — the typed knock
//!
//! *aldrava* (Brazilian-Portuguese: the **door-knocker** — the small fitting
//! you strike to announce yourself, distinct from [`porteiro`](https://github.com/pleme-io/porteiro),
//! the doorman who decides whether to open) generalizes the "leave a
//! `/command` comment on a PR and have a trust-gated pipeline action fire"
//! pattern common to GitHub Actions CI: a comment is a knock; `aldrava`
//! parses it against a registered command catalog, resolves whether the
//! knocker is trusted, and — only when both hold — dispatches a target
//! (relabel, `workflow_dispatch`, or `repository_dispatch`).
//!
//! TYPED-SPEC + INTERPRETER TRIPLET (`pleme-io/CLAUDE.md`):
//! 1. **Rust border** — [`spec`] (the `(defcommentcommand ...)` catalog
//!    shape) + [`event`] (the resolved inbound trigger).
//! 2. **Lisp spec** — [`spec_lisp`], a real parser for a consumer's
//!    `.github/aldrava.lisp` catalog (not documentation parity).
//! 3. **Interpreter** — [`interp::apply`], side effects abstracted behind
//!    the mockable [`environment::Environment`] trait.

pub mod environment;
pub mod event;
pub mod interp;
pub mod spec;
pub mod spec_lisp;
