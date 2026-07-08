;;;; specs/example.lisp — a worked (defcommentcommand ...) catalog.
;;;;
;;;; The Lisp leg of the TYPED-SPEC + INTERPRETER TRIPLET (pleme-io/CLAUDE.md):
;;;;   1. Rust border    — src/spec.rs (CommentCommandSpec / DispatchTarget)
;;;;                        + src/event.rs (InboundEvent).
;;;;   2. Lisp spec       — THIS file: (defcommentcommand ...) forms parsed by
;;;;                        src/spec_lisp.rs into a real CatalogSpec (not
;;;;                        documentation parity — a consuming repo's own
;;;;                        `.github/aldrava.lisp` follows this exact shape).
;;;;   3. Interpreter     — src/interp.rs::apply(catalog, event, env) over the
;;;;                        mockable Environment trait.
;;;;
;;;; A knock is a PR comment starting with a registered :trigger, followed by
;;;; whitespace or end-of-string. The first matching command wins.

;; The simple case: any PR-write-permission collaborator (or the PR's own
;; author) can knock "/test" to relabel the PR and re-fire the heavy test
;; pipeline (which gates on `pull_request: [labeled]` for "ci/run-tests").
(defcommentcommand "test"
  :trigger "/test"
  :min-permission write
  :trust-pr-author true
  :target (label "ci/run-tests"))

;; A second command, same target label — two different knocks converging on
;; one pipeline trigger.
(defcommentcommand "retest"
  :trigger "/retest"
  :min-permission write
  :trust-pr-author true
  :target (label "ci/run-tests"))

;; A higher-trust command that directly fires a workflow_dispatch instead of
;; relabeling — no intermediate `pull_request: [labeled]` hop. Captures the
;; first word after the trigger ("/deploy staging") as $1, and substitutes
;; the resolved branch ref + PR author into the dispatched inputs.
(defcommentcommand "deploy"
  :trigger "/deploy"
  :min-permission admin
  :trust-pr-author false
  :allowlist ("release-bot")
  :target (workflow-dispatch "deploy-environments.yml"
            :ref "$branch-ref"
            :inputs ((environment "$1")
                     (requested-by "$pr-author"))))
