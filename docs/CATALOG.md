# The catalog grammar

A catalog is zero or more `(defcommentcommand ...)` forms — one per
registered `/command`. Parsed by `src/spec_lisp.rs`; see
[`ARCHITECTURE.md`](./ARCHITECTURE.md) for how it fits the typed-spec
pipeline. `specs/example.lisp` is a complete worked file; this page is the
field-by-field reference.

## Shape

```lisp
(defcommentcommand "<name>"
  :trigger "<prefix>"
  :min-permission <permission-symbol>       ; optional, default write
  :trust-pr-author <true|false>             ; optional, default true
  :allowlist ("<login>" "<login>" ...)      ; optional, default ()
  :target (<target-form>))
```

Comments start with `;` and run to end of line. Whitespace is insignificant
between tokens. Strings are double-quoted with `\"`, `\\`, `\n` escapes;
everything else (permission keywords, target-kind heads, `true`/`false`) is
a bare symbol — no quotes.

## Fields

| Field | Required | Type | Default | Meaning |
|---|---|---|---|---|
| name (positional) | yes | string | — | Identifies the command in output/logs. Not matched against the comment — `:trigger` is. |
| `:trigger` | yes | string | — | The literal prefix a comment must start with (e.g. `"/test"`), followed by whitespace or end-of-string. `"/test"` matches `/test` and `/test staging` but never `/testing`. |
| `:min-permission` | no | symbol | `write` | One of `none` \| `read` \| `triage` \| `write` \| `maintain` \| `admin` — the GitHub repo permission tier a commenter must hold, unless trusted some other way. |
| `:trust-pr-author` | no | `true`/`false` | `true` | When true, the PR's own author is always trusted for this command, regardless of `:min-permission`. |
| `:allowlist` | no | list of strings | `()` | Logins trusted for this command regardless of permission or PR authorship — e.g. a bot account with no collaborator record. |
| `:target` | yes | target form | — | What happens on a trusted knock. See below. |

Trust is resolved in that order — author-trust, then allowlist, then
permission — and the first one that matches wins; the others are never
consulted.

## Target forms

### `(label "<name>")`

Remove-then-add `<name>` on the PR — the idempotent relabel that re-fires a
`pull_request: [labeled]`-gated pipeline even when the label is already
present.

```lisp
:target (label "ci/run-tests")
```

### `(workflow-dispatch "<workflow-file>" :ref "<ref>" :inputs ((<key> "<value>") ...))`

Fires a `workflow_dispatch` against `<workflow-file>` on `<ref>`.

| Sub-field | Required | Default |
|---|---|---|
| workflow-file (positional) | yes | — |
| `:ref` | no | the PR's own head branch (`$branch-ref`) |
| `:inputs` | no | `()` |

```lisp
:target (workflow-dispatch "deploy.yml"
          :ref "$branch-ref"
          :inputs ((environment "$1")
                   (requested-by "$pr-author")))
```

### `(repository-dispatch "<event-type>" :repo "<owner/repo>")`

Fires a `repository_dispatch` with `event_type: <event-type>`.

| Sub-field | Required | Default |
|---|---|---|
| event-type (positional) | yes | — |
| `:repo` | no | the current repo |

```lisp
:target (repository-dispatch "run-downstream-suite" :repo "pleme-io/other-repo")
```

## Placeholder substitution

`:ref` and every `:inputs` value in a `workflow-dispatch` target may
reference:

| Placeholder | Value |
|---|---|
| `$checkout-ref` | The PR's head SHA. |
| `$branch-ref` | `refs/heads/<pr-head-ref>`. |
| `$base-ref` | `refs/heads/<pr-base-ref>`. |
| `$pr-author` | The PR author's login. |
| `$1` .. `$9` | Whitespace-split words captured after the trigger (`/deploy staging now` -> `$1` = `staging`, `$2` = `now`). |

A string that isn't one of these is passed through unchanged — a typo in a
placeholder name surfaces in the dispatched value instead of silently
resolving to nothing.

## Multiple commands

A catalog file is a flat sequence of forms; order is preserved but doesn't
affect matching — the first command whose `:trigger` matches the comment
wins, so put a more-specific trigger before a less-specific one that shares
a prefix (`/deploy-prod` before `/deploy` if both exist).

```lisp
(defcommentcommand "test" :trigger "/test" :target (label "ci/run-tests"))
(defcommentcommand "retest" :trigger "/retest" :target (label "ci/run-tests"))
(defcommentcommand "deploy" :trigger "/deploy" :min-permission admin
  :trust-pr-author false
  :target (workflow-dispatch "deploy.yml" :inputs ((environment "$1"))))
```

## Validating a catalog

```
aldrava lint .github/aldrava.lisp
```

Exits 0 with `{"ok": true, "commands": [...]}` on success, or non-zero with
`{"ok": false, "error": "..."}` naming the offending form. Wire this into CI
via the `pleme-io/actions/aldrava-lint` action (see
[`USAGE.md`](./USAGE.md#validating-your-catalog-in-ci)) so a typo in the
catalog fails the PR that introduces it instead of silently no-opping the
next real knock.
