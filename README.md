# aldrava

[![ci](https://github.com/pleme-io/aldrava/actions/workflows/ci.yml/badge.svg)](https://github.com/pleme-io/aldrava/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/pleme-io/aldrava/branch/main/graph/badge.svg)](https://codecov.io/gh/pleme-io/aldrava)
[![crates.io](https://img.shields.io/crates/v/aldrava.svg)](https://crates.io/crates/aldrava)

*aldrava* (Brazilian-Portuguese: the door-knocker) — a typed comment-command
dispatcher for GitHub Actions. A `/command` left on a pull request is a
knock; `aldrava` matches it against a registered command catalog, resolves
whether the knocker is trusted, and — only when both hold — dispatches a
target: an idempotent label relabel, a `workflow_dispatch`, or a
`repository_dispatch`.

Generalizes the common "comment `/test` on a PR, a trusted collaborator's
knock relabels the PR, the relabel re-fires a `pull_request: [labeled]`-gated
heavy pipeline" ChatOps pattern into a small, typed, tested primitive that
any repo can register any number of commands against.

## Quickstart

`.github/workflows/comment-commands.yml`, in the repo you want `/test` to
work on:

```yaml
on:
  issue_comment:
    types: [created]

jobs:
  dispatch:
    uses: pleme-io/substrate/.github/workflows/comment-command-dispatch.yml@main
    with:
      command: test
      target-label: ci/run-tests
    secrets: inherit
```

That's the whole adoption for the common case. See
**[docs/USAGE.md](./docs/USAGE.md)** for multi-command catalogs,
`workflow_dispatch`/`repository_dispatch` targets, wiring the downstream
pipeline, and troubleshooting.

## Docs

| Doc | For |
|---|---|
| **[docs/USAGE.md](./docs/USAGE.md)** | Adopting `aldrava` in a repo — three paths (simple/catalog/direct), the downstream-pipeline half, CI catalog linting, permissions, troubleshooting. |
| **[docs/CATALOG.md](./docs/CATALOG.md)** | The full `(defcommentcommand ...)` grammar reference — every field, every target kind, placeholder substitution. |
| **[docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)** | How it's built — the TYPED-SPEC+INTERPRETER-TRIPLET module map, the CLI's JSON contract, the testing approach, tier-honest notes. |

## CLI

```
aldrava dispatch --catalog .github/aldrava.lisp --repo owner/repo
aldrava dispatch --command test --target-label ci/run-tests   # inline, no catalog file
aldrava resolve --label-name ci/run-tests                     # downstream pipeline context
aldrava lint .github/aldrava.lisp
```

`dispatch`/`resolve` read the event from `$GITHUB_EVENT_PATH`/
`$GITHUB_EVENT_NAME` by default (overridable via `--event-path`/
`--event-name` for local testing) and print one JSON object to stdout — see
[docs/ARCHITECTURE.md#cli-json-contract](./docs/ARCHITECTURE.md#cli-json-contract)
for the exact shape.

## Trust model

A knock is trusted when, in order: the commenter is the PR's own author
(`:trust-pr-author`, default true) OR the commenter's login is in
`:allowlist` OR the commenter holds `:min-permission`+ on the repo (default
`write`). Never over-trusts on ambiguity — an unresolvable PR or an unknown
collaborator permission both resolve to "not trusted," never "trusted by
default."

## Testing

```
cargo test                    # unit + property tests (41 tests, proptest at 256 cases each)
cargo tarpaulin --out Html     # line coverage — see docs/ARCHITECTURE.md for the local-toolchain note
```

Coverage runs on every push/PR via `pleme-io/actions/coverage-upload` ->
Codecov. Full breakdown of what's tested and why in
[docs/ARCHITECTURE.md#testing](./docs/ARCHITECTURE.md#testing).

## Related

- [`pleme-io/actions`](https://github.com/pleme-io/actions) —
  `aldrava-dispatch`, `aldrava-resolve`, `aldrava-lint` (the GitHub Actions
  that wrap this CLI).
- [`pleme-io/substrate`](https://github.com/pleme-io/substrate) —
  `comment-command-dispatch.yml` (the reusable workflow the Quickstart
  above calls).

## License

MIT.
