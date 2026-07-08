//! Property tests over `interp::apply` — the trust + dispatch decision. Each
//! property pins one invariant the interpreter must never violate for *any*
//! input in its domain, not just the hand-picked examples in `src/interp.rs`'s
//! own unit tests.

use aldrava::environment::{MockEnvironment, PullRequestInfo};
use aldrava::event::InboundEvent;
use aldrava::interp::{DispatchOutcome, apply};
use aldrava::spec::{CatalogSpec, CommentCommandSpec, DispatchTarget, Permission};
use proptest::prelude::*;

fn arg_word() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_.-]{1,10}"
}

fn label_command(
    trust_pr_author: bool,
    min_permission: Permission,
    allowlist: Vec<String>,
) -> CommentCommandSpec {
    CommentCommandSpec {
        name: "cmd".to_string(),
        trigger: "/cmd".to_string(),
        min_permission,
        trust_pr_author,
        allowlist,
        target: DispatchTarget::Label {
            name: "lbl".to_string(),
        },
    }
}

fn pr(author: &str) -> PullRequestInfo {
    PullRequestInfo {
        author: author.to_string(),
        head_sha: "sha".to_string(),
        head_ref: "feature".to_string(),
        base_ref: "main".to_string(),
    }
}

const ALL_PERMISSIONS: [Permission; 6] = [
    Permission::None,
    Permission::Read,
    Permission::Triage,
    Permission::Write,
    Permission::Maintain,
    Permission::Admin,
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever whitespace-free words follow the trigger are captured
    /// verbatim and in order as `args`, for any arg count.
    #[test]
    fn args_after_trigger_round_trip(args in proptest::collection::vec(arg_word(), 0..6)) {
        let catalog = CatalogSpec::single(label_command(true, Permission::Write, vec![]));
        let body = if args.is_empty() { "/cmd".to_string() } else { format!("/cmd {}", args.join(" ")) };
        let env = MockEnvironment::new().with_pull_request(1, pr("alice"));
        let event = InboundEvent::IssueComment {
            issue_number: 1,
            comment_body: body,
            commenter_login: "alice".to_string(),
        };
        let outcome = apply(&catalog, &event, &env).unwrap();
        match outcome {
            DispatchOutcome::Dispatched { args: got, .. } => prop_assert_eq!(got, args),
            other => prop_assert!(false, "expected Dispatched, got {other:?}"),
        }
    }

    /// `trust_pr_author = true` + commenter is the PR's own author always
    /// dispatches, for every `min_permission` tier and every allowlist —
    /// permission/allowlist are never consulted once author-trust holds.
    #[test]
    fn pr_author_trust_wins_regardless_of_permission_or_allowlist(
        perm_idx in 0..ALL_PERMISSIONS.len(),
        allowlist in proptest::collection::vec(arg_word(), 0..3),
    ) {
        let catalog = CatalogSpec::single(label_command(true, ALL_PERMISSIONS[perm_idx], allowlist));
        // "alice" has no permission entry in the mock env at all (Permission::None).
        let env = MockEnvironment::new().with_pull_request(1, pr("alice"));
        let event = InboundEvent::IssueComment {
            issue_number: 1,
            comment_body: "/cmd".to_string(),
            commenter_login: "alice".to_string(),
        };
        let outcome = apply(&catalog, &event, &env).unwrap();
        prop_assert!(outcome.dispatched(), "PR author must always be trusted, got {outcome:?}");
    }

    /// A commenter who is neither the PR author, allowlisted, nor holding
    /// `min_permission`+ never causes a mutation — rejection is silent on
    /// the environment, not just on the returned outcome.
    #[test]
    fn untrusted_knock_never_mutates_labels(commenter in "[a-z][a-z0-9]{1,8}") {
        prop_assume!(commenter != "alice");
        let catalog = CatalogSpec::single(label_command(true, Permission::Write, vec![]));
        let env = MockEnvironment::new().with_pull_request(1, pr("alice"));
        let event = InboundEvent::IssueComment {
            issue_number: 1,
            comment_body: "/cmd".to_string(),
            commenter_login: commenter,
        };
        let outcome = apply(&catalog, &event, &env).unwrap();
        prop_assert!(!outcome.dispatched());
        prop_assert!(env.labels_added.borrow().is_empty());
        prop_assert!(env.labels_removed.borrow().is_empty());
    }

    /// A commenter holding at least `min_permission` is always trusted,
    /// regardless of PR authorship or allowlist membership — for every
    /// permission tier at or above the command's minimum.
    #[test]
    fn permission_at_or_above_minimum_is_always_trusted(
        min_idx in 0..ALL_PERMISSIONS.len(),
        held_idx in 0..ALL_PERMISSIONS.len(),
    ) {
        prop_assume!(held_idx >= min_idx);
        let catalog = CatalogSpec::single(label_command(false, ALL_PERMISSIONS[min_idx], vec![]));
        let env = MockEnvironment::new()
            .with_pull_request(1, pr("alice"))
            .with_permission("bob", ALL_PERMISSIONS[held_idx]);
        let event = InboundEvent::IssueComment {
            issue_number: 1,
            comment_body: "/cmd".to_string(),
            commenter_login: "bob".to_string(),
        };
        let outcome = apply(&catalog, &event, &env).unwrap();
        prop_assert!(outcome.dispatched(), "bob holds >= min_permission, must be trusted, got {outcome:?}");
    }

    /// A commenter holding strictly less than `min_permission`, who is
    /// neither the author nor allowlisted, is always rejected.
    #[test]
    fn permission_below_minimum_is_always_rejected(
        min_idx in 1..ALL_PERMISSIONS.len(),
        held_idx in 0..ALL_PERMISSIONS.len(),
    ) {
        prop_assume!(held_idx < min_idx);
        let catalog = CatalogSpec::single(label_command(false, ALL_PERMISSIONS[min_idx], vec![]));
        let env = MockEnvironment::new()
            .with_pull_request(1, pr("alice"))
            .with_permission("bob", ALL_PERMISSIONS[held_idx]);
        let event = InboundEvent::IssueComment {
            issue_number: 1,
            comment_body: "/cmd".to_string(),
            commenter_login: "bob".to_string(),
        };
        let outcome = apply(&catalog, &event, &env).unwrap();
        prop_assert!(!outcome.dispatched(), "bob holds < min_permission, must be rejected, got {outcome:?}");
    }

    /// A comment whose trigger prefix matches but is immediately followed
    /// by a non-whitespace character (e.g. `/cmdx`) never matches — the
    /// boundary check holds for any suffix, not just the hand-picked
    /// `/testing` example.
    #[test]
    fn trigger_prefix_without_word_boundary_never_matches(suffix in "[a-zA-Z0-9]{1,8}") {
        let catalog = CatalogSpec::single(label_command(true, Permission::Write, vec![]));
        let env = MockEnvironment::new().with_pull_request(1, pr("alice"));
        let event = InboundEvent::IssueComment {
            issue_number: 1,
            comment_body: format!("/cmd{suffix}"),
            commenter_login: "alice".to_string(),
        };
        let outcome = apply(&catalog, &event, &env).unwrap();
        let is_no_match = matches!(outcome, DispatchOutcome::NoMatch { .. });
        prop_assert!(is_no_match);
    }
}
