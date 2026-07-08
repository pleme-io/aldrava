# aldrava

*aldrava* (Brazilian-Portuguese: the door-knocker) — a typed comment-command
dispatcher for GitHub Actions. A `/command` left on a pull request is a
knock; `aldrava` matches it against a registered command catalog, resolves
whether the knocker is trusted, and — only when both hold — dispatches a
target: an idempotent label relabel, a `workflow_dispatch`, or a
`repository_dispatch`.

Generalizes the common "comment `/test` on a PR, a trusted collaborator's
knock relabels the PR, the relabel re-fires a `pull_request: [labeled]`-gated
heavy pipeline" ChatOps pattern into a small, typed, unit-tested primitive
that any repo can register any number of commands against.

## Design

Follows the TYPED-SPEC + INTERPRETER TRIPLET:

1. **Rust border** (`src/spec.rs`, `src/event.rs`) — `CommentCommandSpec`,
   `Permission`, `DispatchTarget`, and the resolved `InboundEvent` shape.
2. **Lisp spec** (`src/spec_lisp.rs`) — a real parser (not documentation
   parity) for a consuming repo's `.github/aldrava.lisp` catalog:

   ```lisp
   (defcommentcommand "test"
     :trigger "/test"
     :min-permission write
     :trust-pr-author true
     :target (label "ci/run-tests"))
   ```

   See `specs/example.lisp` for the full grammar (label /
   workflow-dispatch / repository-dispatch targets, `:allowlist`,
   placeholder substitution in workflow-dispatch inputs).
3. **Interpreter** (`src/interp.rs`) — `apply(catalog, event, env)`. Side
   effects (permission lookups, PR fetch, label/dispatch mutation) sit behind
   the `Environment` trait (`src/environment.rs`), so the full decision logic
   is tested with zero network access via `MockEnvironment`.

## CLI

```
aldrava dispatch --catalog .github/aldrava.lisp --repo owner/repo
aldrava dispatch --command test --target-label ci/run-tests   # inline, no catalog file
aldrava resolve --label-name ci/run-tests                     # downstream pipeline context
aldrava lint .github/aldrava.lisp
```

`dispatch`/`resolve` read the event from `$GITHUB_EVENT_PATH`/
`$GITHUB_EVENT_NAME` by default (overridable via `--event-path`/
`--event-name` for local testing) and print one JSON object to stdout.

## Trust model

A knock is trusted when, in order: the commenter is the PR's own author
(`:trust-pr-author`, default true) OR the commenter's login is in
`:allowlist` OR the commenter holds `:min-permission`+ on the repo (default
`write`). Never over-trusts on ambiguity — an unresolvable PR or an unknown
collaborator permission both resolve to "not trusted," never "trusted by
default."

## Status

Tier-honest: `spec_lisp.rs` is `aldrava`'s own minimal S-expression reader
for its one grammar, not the shared `tatara_lisp` crate's
`#[derive(TataraDomain)]` registration machinery — that surface is not yet a
runtime-consumable parsing library for external crates. Swapping the reader
for `tatara_lisp::domain::register::<CatalogSpec>()` once that ships is a
named, isolated follow-up; the authoring vocabulary already matches the
fleet's `(def<thing> "name" :key value ...)` convention.
