# Usage

Three ways to adopt `aldrava`, from least to most configuration. Start with
Path A; move to B or C only when you actually need what they add.

- [Path A — one command, via the reusable workflow](#path-a--one-command-via-the-reusable-workflow)
- [Path B — a catalog, for multiple commands or non-label targets](#path-b--a-catalog-for-multiple-commands-or-non-label-targets)
- [Path C — the actions directly, for a custom job shape](#path-c--the-actions-directly-for-a-custom-job-shape)
- [The downstream pipeline: `aldrava-resolve`](#the-downstream-pipeline-aldrava-resolve)
- [Validating your catalog in CI](#validating-your-catalog-in-ci)
- [Permissions](#permissions)
- [Troubleshooting](#troubleshooting)

## Path A — one command, via the reusable workflow

This is the shape of the common "comment `/test`, a trusted collaborator's
knock relabels the PR, the relabel re-fires the test pipeline" pattern.
Nothing to author beyond the workflow file itself — no catalog, no Lisp.

`.github/workflows/comment-commands.yml`:

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

Your existing heavy pipeline just needs to gate on the label and resolve its
own context uniformly (see
[below](#the-downstream-pipeline-aldrava-resolve)):

```yaml
# .github/workflows/pipeline.yml (your existing heavy CI)
on:
  pull_request:
    types: [labeled]
  workflow_dispatch:
  schedule:
    - cron: "0 6 * * *"

jobs:
  resolve:
    runs-on: ubuntu-latest
    outputs:
      should-run: ${{ steps.ctx.outputs.should-run }}
      checkout-ref: ${{ steps.ctx.outputs.checkout-ref }}
    steps:
      - id: ctx
        uses: pleme-io/actions/aldrava-resolve@main
        with:
          label-name: ci/run-tests

  build:
    needs: resolve
    if: needs.resolve.outputs.should-run == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ needs.resolve.outputs.checkout-ref }}
      # ... your real pipeline ...
```

That's the whole adoption. `/test` on a PR from a trusted commenter (the
PR's own author, or anyone holding `write`+ on the repo) relabels
`ci/run-tests`, which re-fires `pipeline.yml`; an untrusted commenter's
`/test` is silently ignored (no mutation, no error).

## Path B — a catalog, for multiple commands or non-label targets

Once you need more than one command, or a `workflow_dispatch`/
`repository_dispatch` target, author `.github/aldrava.lisp` — see
[`CATALOG.md`](./CATALOG.md) for the full grammar:

```lisp
;; .github/aldrava.lisp
(defcommentcommand "test"
  :trigger "/test"
  :target (label "ci/run-tests"))

(defcommentcommand "deploy"
  :trigger "/deploy"
  :min-permission admin
  :trust-pr-author false
  :allowlist ("release-bot")
  :target (workflow-dispatch "deploy.yml"
            :ref "$branch-ref"
            :inputs ((environment "$1")
                     (requested-by "$pr-author"))))
```

Point the reusable workflow at it:

```yaml
jobs:
  dispatch:
    uses: pleme-io/substrate/.github/workflows/comment-command-dispatch.yml@main
    with:
      catalog-path: .github/aldrava.lisp
    secrets: inherit
```

`catalog-path` takes over entirely — `command`/`target-*` inputs are
ignored when it's set. `/deploy staging` from `release-bot` (or any repo
admin) now fires `deploy.yml` on the PR's branch with `environment=staging`
— no relabel hop, no downstream pipeline to gate.

## Path C — the actions directly, for a custom job shape

The reusable workflow is `pleme-io/actions/aldrava-dispatch` behind a cheap
`startsWith(comment.body, '/')` YAML-level pre-filter (so an unrelated
comment never spins a runner) plus fixed `permissions:`. Use the action
directly when you need a different filter, different permissions, or extra
steps around the dispatch:

```yaml
on:
  issue_comment:
    types: [created]

permissions:
  contents: read
  pull-requests: write
  issues: write
  actions: write   # only if you use workflow-dispatch/repository-dispatch targets

jobs:
  dispatch:
    if: github.event.issue.pull_request
    runs-on: ubuntu-latest
    steps:
      - id: knock
        uses: pleme-io/actions/aldrava-dispatch@main
        with:
          catalog-path: .github/aldrava.lisp
      - if: steps.knock.outputs.outcome == 'rejected'
        run: echo "an untrusted knock was rejected — ${{ steps.knock.outputs.reason }}"
```

Every `aldrava-dispatch` output (`dispatched`, `outcome`, `command`, `args`,
`commenter`, `checkout-ref`, `branch-ref`, `base-ref`, `pr-author`,
`is-develop`, `target-kind`, `target-detail`, `reason`) is available for
downstream steps — see [`ARCHITECTURE.md`](./ARCHITECTURE.md#cli-json-contract)
for the exact shape each field carries.

## The downstream pipeline: `aldrava-resolve`

A label-target knock re-triggers your heavy pipeline via
`pull_request: [labeled]` — but that pipeline might *also* run on
`workflow_dispatch`, `schedule`, or `repository_dispatch`, each of which
carries a different event shape. `aldrava-resolve` normalizes all of them
into one context (`should-run`, `checkout-ref`, `branch-ref`, `base-ref`,
`pr-author`, `is-develop`) so the rest of the pipeline doesn't branch on
`github.event_name` itself:

```yaml
- id: ctx
  uses: pleme-io/actions/aldrava-resolve@main
  with:
    label-name: ci/run-tests   # omit to accept every pull_request action
```

`should-run` is `false` (and every other output empty) when: the event is
`pull_request: [labeled]` with a *different* label, or an event source
`aldrava-resolve` has no context for (e.g. `issue_comment` — that's
`aldrava-dispatch`'s job, not this one's).

## Validating your catalog in CI

```yaml
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pleme-io/actions/aldrava-lint@main
        with:
          catalog-path: .github/aldrava.lisp
```

Fails the step (with the offending form named) on a malformed catalog —
catches a typo'd keyword or missing `:target` at PR time instead of at the
next real knock.

## Permissions

| Target kind | Minimum `permissions:` |
|---|---|
| `label` | `pull-requests: write`, `issues: write` |
| `workflow-dispatch` | `actions: write` |
| `repository-dispatch` | `actions: write` (and a token with access to the target repo, if cross-repo) |

`aldrava-dispatch`'s default `token` input is `${{ github.token }}` — the
job's own `GITHUB_TOKEN`, scoped by the workflow's `permissions:` block.
Override `token` (and grant a broader-scoped PAT via `secrets:`) for
cross-repo `repository-dispatch` targets, since `GITHUB_TOKEN` never has
access outside its own repo.

## Troubleshooting

**A trusted `/test` comment does nothing.** Check `outcome` — `no-match`
means the trigger didn't match (mind the word-boundary: `/test` doesn't
match `/testing`); `rejected` means the commenter wasn't trusted (check
`reason`); `not-on-pull-request` means the comment landed on a plain issue,
not a PR.

**The relabel doesn't re-trigger the downstream pipeline.** Confirm the
downstream workflow's `on:` actually includes `pull_request: { types:
[labeled] }` — GitHub only fires that event on a label *transition*
(absent -> present), which is exactly why `aldrava` removes-then-adds
rather than just adding.

**`workflow_dispatch` target returns `dispatched: true` but nothing runs.**
The target workflow file must itself declare `on: workflow_dispatch:` —
`aldrava` calls the GitHub API dispatch endpoint, which 404s (surfaced as a
`SpecError` and a non-zero exit) if the workflow doesn't accept that
trigger.

**Testing locally without a real PR comment.** `aldrava dispatch
--event-path <file> --event-name issue_comment --catalog .github/aldrava.lisp`
against a hand-written event JSON — see `tests/property_interp.rs` in this
repo for the minimal payload shape each event kind needs.
