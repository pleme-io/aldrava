# Architecture

## The TYPED-SPEC + INTERPRETER TRIPLET

`aldrava` follows the fleet's canonical shape for "do these steps in this
order against these inputs" logic (`pleme-io/CLAUDE.md`): a typed Rust
border, an authored Lisp spec, and an interpreter whose side effects sit
behind a mockable trait.

```
   .github/aldrava.lisp                    issue_comment / pull_request /
   (defcommentcommand ...)                  workflow_dispatch / ... event
            |                                          |
            v                                          v
      spec_lisp::parse                          event::resolve
            |                                          |
            v                                          v
      spec::CatalogSpec                        event::InboundEvent
            \_______________________  ________________/
                                    \/
                              interp::apply(catalog, event, env)
                                    |
                    (matches a command? is the knocker trusted?)
                                    |
                    +---------------+---------------+
                    |                                |
              DispatchOutcome                  environment::Environment
          (NotOnPullRequest | NoMatch |          (mutates GitHub — label
           Rejected | Dispatched)                 relabel / workflow_dispatch
                    |                              / repository_dispatch)
                    v
              src/main.rs CLI
           (JSON on stdout/stderr)
                    |
                    v
     pleme-io/actions/aldrava-{dispatch,resolve,lint}
              (run.tlisp: exec-capture + json-read-field)
                    |
                    v
    pleme-io/substrate/comment-command-dispatch.yml
         (the reusable workflow a consumer repo calls)
```

## Module map

| Module | Owns |
|---|---|
| `src/spec.rs` | The Rust border: `CommentCommandSpec`, `Permission` (ordered — `PartialOrd`/`Ord` derive from declaration order, so `commenter >= min_permission` is a typed comparison), `DispatchTarget`, `CatalogSpec`. |
| `src/event.rs` | `InboundEvent` (the resolved trigger shape, one variant per event kind `aldrava` understands) and `RunContext` (the downstream-pipeline-facing uniform context). `resolve(event_name, payload)` reads only the specific JSON pointers each shape needs — it does not attempt to model the full GitHub webhook schema. |
| `src/spec_lisp.rs` | A hand-rolled recursive-descent S-expression reader for the one `(defcommentcommand ...)` grammar — see [tier-honest note](#tier-honest-notes) below. Tokenizer -> `SExpr` -> semantic walk -> `CatalogSpec`. |
| `src/environment.rs` | The `Environment` trait (`collaborator_permission`, `get_pull_request`, `add_label`/`remove_label` (+ a default `relabel` = remove-then-add), `dispatch_workflow`, `repository_dispatch`) plus two implementations: `MockEnvironment` (in-memory, records every call, zero network — what every test drives against) and `GitHubEnvironment` (the real GitHub REST API over `ureq`). |
| `src/interp.rs` | `apply(catalog, event, env) -> Result<DispatchOutcome, SpecError>` — the decision. Matches the comment against the catalog's triggers (prefix + word-boundary), resolves trust (author -> allowlist -> permission, first match wins, cheapest check first), and — only when both hold — calls the `Environment` to execute the target, substituting `$checkout-ref`/`$branch-ref`/`$base-ref`/`$pr-author`/`$1..$9` placeholders along the way. |
| `src/main.rs` | The CLI (`clap`): `dispatch` (resolve + execute side effects), `resolve` (context-only, no network), `lint` (parse-and-validate). Prints one JSON object to stdout on success, or `{"ok": false, "error": ...}` to stderr with exit 1 on a genuine failure — the shape `run.tlisp` on the Action side parses. |

## CLI JSON contract

This is the interface `pleme-io/actions/aldrava-{dispatch,resolve,lint}`'s
`run.tlisp` depends on — changing a field name here is a breaking change to
those actions' outputs.

### `aldrava dispatch`

One of, on stdout, exit 0:

```jsonc
{"dispatched": false, "outcome": "not-on-pull-request"}
{"dispatched": false, "outcome": "no-match", "reason": "..."}
{"dispatched": false, "outcome": "rejected", "command": "...", "commenter": "...", "reason": "..."}
{
  "dispatched": true, "outcome": "dispatched",
  "command": "...", "args": ["..."], "commenter": "...",
  "checkout_ref": "...", "branch_ref": "...", "base_ref": "...", "pr_author": "...",
  "is_develop": true, "target_kind": "label", "target_detail": "ci/run-tests"
}
```

On a genuine failure (malformed inputs, or the trusted target's own GitHub
API mutation failed) — stderr, exit 1: `{"ok": false, "error": "..."}`.

### `aldrava resolve`

Always stdout, exit 0 (a "shouldn't run" verdict is a normal result, not an
error):

```jsonc
{
  "should_run": false, "checkout_ref": "", "branch_ref": "", "base_ref": "",
  "pr_author": "", "is_develop": false, "reason": "label `x` does not match expected `y`"
}
```

### `aldrava lint`

```jsonc
{"ok": true, "commands": ["test", "deploy"]}      // stdout, exit 0
{"ok": false, "error": "in `defcommentcommand \"x\"`: missing required `:target`"}  // stderr, exit 1
```

## Testing

Two layers (see the [README](../README.md#testing) for the exact
commands):

- **Unit tests** — `#[cfg(test)] mod tests` in each `src/*.rs`, one
  hand-picked example per specific behavior.
- **Property tests** — `tests/property_*.rs`, `proptest`, 256 cases each
  (the fleet floor is 100 — see the `compiler-verifier` skill). Pin the
  invariants a finite set of examples can't: the parser never panics on
  arbitrary bytes; trust resolution is monotonic in `Permission` (every
  tier at-or-above the minimum is trusted, every tier below is rejected,
  for all six tiers); an untrusted knock never calls a mutating
  `Environment` method; args round-trip for any word count; the trigger's
  word-boundary check holds for any suffix.

Coverage: `pleme-io/actions/coverage-upload` (`cargo-tarpaulin` ->
Codecov, tokenless for this public repo) runs in `.github/workflows/ci.yml`
on every push/PR.

## Tier-honest notes

- `spec_lisp.rs` is `aldrava`'s own minimal S-expression reader for its one
  grammar, not the shared `tatara_lisp` crate's `#[derive(TataraDomain)]`
  registration machinery — that surface is not yet a runtime-consumable
  parsing library for external crates (verified against the two most
  recent TYPED-SPEC+INTERPRETER-TRIPLET references in the fleet, neither of
  which depends on it either). Swapping the reader for
  `tatara_lisp::domain::register::<CatalogSpec>()` once that ships is a
  named, isolated follow-up — the authoring vocabulary already matches the
  fleet's `(def<thing> "name" :key value ...)` convention, so the swap
  changes only this file.
- `event::resolve` reads specific JSON pointers, not a fully-typed GitHub
  webhook schema. Adding a new event source `aldrava` should understand
  means adding a match arm and the pointers it needs — not modeling more of
  the webhook payload than that.
- `auto-release.yml` (bump -> tag -> publish) is not gated on `ci.yml`
  passing — they're independent `on: push` triggers, matching the fleet-
  wide `cargo-auto-release.yml` convention (no repo in the fleet gates its
  auto-release on its own CI). In practice the bump step pulls latest
  `main` before committing, so a version tag reflects whatever landed most
  recently — but a red `ci` run and a successful `auto-release` run can be
  in flight at the same time. Tightening this (gate `auto-release` on
  `ci`'s conclusion) would be a fleet-wide change, out of scope here.
