//! The spec interpreter — `apply(catalog, event, env) -> Result<DispatchOutcome, SpecError>`.
//!
//! Completes the TYPED-SPEC + INTERPRETER TRIPLET: the Rust border
//! ([`crate::spec`] / [`crate::event`]), the authored Lisp spec
//! ([`crate::spec_lisp`]), and this interpreter. Every phase is walked in
//! order and every branch returns a named, typed [`DispatchOutcome`] variant
//! — there is no silent "did nothing" path; an untrusted knock, an unmatched
//! command, and a successful dispatch are three different, equally-explicit
//! outcomes a caller can render or assert against.

use std::collections::BTreeMap;

use crate::environment::{EnvError, Environment};
use crate::event::{InboundEvent, is_develop_ref, to_branch_ref};
use crate::spec::{CatalogSpec, CommentCommandSpec, DispatchTarget};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum SpecError {
    #[error("interpreter phase `{phase}` failed: {source}")]
    Interp {
        phase: &'static str,
        #[source]
        source: EnvError,
    },
}

/// The trust + dispatch context resolved from the PR the knock landed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContext {
    pub checkout_ref: String,
    pub branch_ref: String,
    pub base_ref: String,
    pub pr_author: String,
    pub is_develop: bool,
}

/// The interpreter's outcome — one explicit variant per phase a run can end
/// at. Never conflates "no command matched" with "matched but untrusted"
/// with "matched, trusted, dispatched" — each is independently observable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The event was an `issue_comment` on a plain issue, not a PR — never a
    /// knock target.
    NotOnPullRequest,
    /// The event carried no command the catalog recognizes (including: not
    /// an `issue_comment` event at all).
    NoMatch { reason: String },
    /// A command matched, but the commenter was not trusted to run it.
    Rejected {
        command: String,
        commenter: String,
        reason: String,
    },
    /// A command matched, the commenter was trusted, and the target was
    /// dispatched.
    Dispatched {
        command: String,
        args: Vec<String>,
        commenter: String,
        context: ResolvedContext,
        target_kind: &'static str,
        target_detail: String,
    },
}

impl DispatchOutcome {
    #[must_use]
    pub fn dispatched(&self) -> bool {
        matches!(self, Self::Dispatched { .. })
    }
}

/// Match `body` against `trigger` as a prefix followed by whitespace-or-EOL
/// (mirrors `/^\/test(\s|$)/`). Returns the whitespace-split remainder as
/// captured args on a match.
fn match_trigger<'a>(body: &'a str, trigger: &str) -> Option<Vec<&'a str>> {
    let rest = body.strip_prefix(trigger)?;
    match rest.chars().next() {
        None => Some(Vec::new()),
        Some(c) if c.is_whitespace() => Some(rest.split_whitespace().collect()),
        Some(_) => None,
    }
}

fn find_command<'a>(
    catalog: &'a CatalogSpec,
    body: &str,
) -> Option<(&'a CommentCommandSpec, Vec<String>)> {
    for cmd in &catalog.commands {
        if let Some(args) = match_trigger(body.trim_start(), &cmd.trigger) {
            return Some((cmd, args.into_iter().map(str::to_string).collect()));
        }
    }
    None
}

fn is_trusted(
    spec: &CommentCommandSpec,
    commenter: &str,
    pr_author: &str,
    env: &dyn Environment,
) -> bool {
    if spec.trust_pr_author && commenter == pr_author {
        return true;
    }
    if spec.allowlist.iter().any(|a| a == commenter) {
        return true;
    }
    env.collaborator_permission(commenter) >= spec.min_permission
}

/// Substitute `$checkout-ref` / `$branch-ref` / `$base-ref` / `$pr-author` /
/// `$1`.."$9" placeholders in a target template string from the resolved
/// context and captured args. An unrecognized `$...` token is left verbatim
/// — never silently dropped, so a typo in a consumer's catalog surfaces in
/// the dispatched value instead of vanishing.
fn substitute(template: &str, ctx: &ResolvedContext, args: &[String]) -> String {
    match template {
        "$checkout-ref" => ctx.checkout_ref.clone(),
        "$branch-ref" => ctx.branch_ref.clone(),
        "$base-ref" => ctx.base_ref.clone(),
        "$pr-author" => ctx.pr_author.clone(),
        _ => {
            if let Some(digit) = template
                .strip_prefix('$')
                .and_then(|d| d.parse::<usize>().ok())
                && digit >= 1
            {
                return args.get(digit - 1).cloned().unwrap_or_default();
            }
            template.to_string()
        }
    }
}

fn substitute_inputs(
    inputs: &BTreeMap<String, String>,
    ctx: &ResolvedContext,
    args: &[String],
) -> BTreeMap<String, String> {
    inputs
        .iter()
        .map(|(k, v)| (k.clone(), substitute(v, ctx, args)))
        .collect()
}

fn dispatch_target(
    target: &DispatchTarget,
    issue_number: u64,
    ctx: &ResolvedContext,
    args: &[String],
    env: &dyn Environment,
) -> Result<(&'static str, String), SpecError> {
    match target {
        DispatchTarget::Label { name } => {
            env.relabel(issue_number, name)
                .map_err(|source| SpecError::Interp {
                    phase: "relabel",
                    source,
                })?;
            Ok(("label", name.clone()))
        }
        DispatchTarget::WorkflowDispatch {
            workflow,
            git_ref,
            inputs,
        } => {
            let resolved_ref = git_ref
                .as_deref()
                .map_or_else(|| ctx.branch_ref.clone(), |r| substitute(r, ctx, args));
            let resolved_inputs = substitute_inputs(inputs, ctx, args);
            env.dispatch_workflow(workflow, &resolved_ref, &resolved_inputs)
                .map_err(|source| SpecError::Interp {
                    phase: "workflow-dispatch",
                    source,
                })?;
            Ok(("workflow-dispatch", workflow.clone()))
        }
        DispatchTarget::RepositoryDispatch { event_type, repo } => {
            let payload = serde_json::json!({
                "checkout_ref": ctx.checkout_ref,
                "branch_ref": ctx.branch_ref,
                "base_ref": ctx.base_ref,
                "pr_author": ctx.pr_author,
                "args": args,
            });
            env.repository_dispatch(event_type, repo.as_deref(), &payload)
                .map_err(|source| SpecError::Interp {
                    phase: "repository-dispatch",
                    source,
                })?;
            Ok(("repository-dispatch", event_type.clone()))
        }
    }
}

/// Walk the catalog against one resolved [`InboundEvent`]: match a command,
/// resolve trust, and — only when both succeed — dispatch the target.
///
/// # Errors
/// [`SpecError::Interp`] when a trusted knock's target mutation itself fails
/// (e.g. the GitHub API rejects the relabel/dispatch call). Every other
/// branch — no match, untrusted, not-a-PR — is a typed [`DispatchOutcome`],
/// never an error.
pub fn apply(
    catalog: &CatalogSpec,
    event: &InboundEvent,
    env: &dyn Environment,
) -> Result<DispatchOutcome, SpecError> {
    let InboundEvent::IssueComment {
        issue_number,
        comment_body,
        commenter_login,
    } = event
    else {
        return Ok(match event {
            InboundEvent::IssueCommentNotOnPullRequest => DispatchOutcome::NotOnPullRequest,
            other => DispatchOutcome::NoMatch {
                reason: format!("event is not an issue_comment on a pull request ({other:?})"),
            },
        });
    };

    let Some((cmd, args)) = find_command(catalog, comment_body) else {
        return Ok(DispatchOutcome::NoMatch {
            reason: "comment does not match any registered command trigger".to_string(),
        });
    };

    let Some(pr) = env.get_pull_request(*issue_number) else {
        return Ok(DispatchOutcome::Rejected {
            command: cmd.name.clone(),
            commenter: commenter_login.clone(),
            reason: format!("could not resolve pull request #{issue_number}"),
        });
    };

    if !is_trusted(cmd, commenter_login, &pr.author, env) {
        return Ok(DispatchOutcome::Rejected {
            command: cmd.name.clone(),
            commenter: commenter_login.clone(),
            reason: format!(
                "commenter `{commenter_login}` is not the PR author, not allowlisted, and does not hold `{:?}`+ permission",
                cmd.min_permission
            ),
        });
    }

    let branch_ref = to_branch_ref(&pr.head_ref);
    let ctx = ResolvedContext {
        checkout_ref: pr.head_sha.clone(),
        is_develop: is_develop_ref(&branch_ref),
        branch_ref,
        base_ref: to_branch_ref(&pr.base_ref),
        pr_author: pr.author.clone(),
    };

    let (target_kind, target_detail) =
        dispatch_target(&cmd.target, *issue_number, &ctx, &args, env)?;

    Ok(DispatchOutcome::Dispatched {
        command: cmd.name.clone(),
        args,
        commenter: commenter_login.clone(),
        context: ctx,
        target_kind,
        target_detail,
    })
}

#[cfg(test)]
mod tests {
    use super::{DispatchOutcome, apply};
    use crate::environment::{MockEnvironment, PullRequestInfo};
    use crate::event::InboundEvent;
    use crate::spec::{CatalogSpec, CommentCommandSpec, DispatchTarget, Permission};

    fn label_command(name: &str, trigger: &str) -> CommentCommandSpec {
        CommentCommandSpec {
            name: name.to_string(),
            trigger: trigger.to_string(),
            min_permission: Permission::Write,
            trust_pr_author: true,
            allowlist: Vec::new(),
            target: DispatchTarget::Label {
                name: "ci/run-tests".to_string(),
            },
        }
    }

    fn pr(author: &str) -> PullRequestInfo {
        PullRequestInfo {
            author: author.to_string(),
            head_sha: "deadbeef".to_string(),
            head_ref: "feature-x".to_string(),
            base_ref: "develop".to_string(),
        }
    }

    fn comment(body: &str, commenter: &str) -> InboundEvent {
        InboundEvent::IssueComment {
            issue_number: 7,
            comment_body: body.to_string(),
            commenter_login: commenter.to_string(),
        }
    }

    #[test]
    fn trusted_pr_author_dispatches_and_relabels() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("/test", "alice"), &env).unwrap();
        assert!(outcome.dispatched());
        assert_eq!(
            env.labels_added.borrow().as_slice(),
            &[(7, "ci/run-tests".to_string())]
        );
        assert_eq!(
            env.labels_removed.borrow().as_slice(),
            &[(7, "ci/run-tests".to_string())]
        );
        let DispatchOutcome::Dispatched { context, .. } = outcome else {
            unreachable!()
        };
        // is_develop tracks the PR's own HEAD ref ("feature-x"), not its base.
        assert!(!context.is_develop);
        assert_eq!(context.checkout_ref, "deadbeef");
    }

    #[test]
    fn untrusted_commenter_below_min_permission_is_rejected() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new()
            .with_pull_request(7, pr("alice"))
            .with_permission("mallory", Permission::Read);
        let outcome = apply(&catalog, &comment("/test", "mallory"), &env).unwrap();
        assert!(matches!(outcome, DispatchOutcome::Rejected { .. }));
        assert!(
            env.labels_added.borrow().is_empty(),
            "must never mutate on an untrusted knock"
        );
    }

    #[test]
    fn write_permission_commenter_who_is_not_the_author_is_trusted() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new()
            .with_pull_request(7, pr("alice"))
            .with_permission("bob", Permission::Write);
        let outcome = apply(&catalog, &comment("/test", "bob"), &env).unwrap();
        assert!(outcome.dispatched());
    }

    #[test]
    fn allowlisted_login_is_trusted_regardless_of_permission() {
        let mut cmd = label_command("test", "/test");
        cmd.trust_pr_author = false;
        cmd.min_permission = Permission::Admin;
        cmd.allowlist = vec!["release-bot".to_string()];
        let catalog = CatalogSpec::single(cmd);
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("/test", "release-bot"), &env).unwrap();
        assert!(outcome.dispatched());
    }

    #[test]
    fn unknown_command_is_no_match() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("/bogus", "alice"), &env).unwrap();
        assert!(matches!(outcome, DispatchOutcome::NoMatch { .. }));
    }

    #[test]
    fn trigger_prefix_without_boundary_does_not_match() {
        // "/testing" must NOT match a "/test" trigger.
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("/testing something", "alice"), &env).unwrap();
        assert!(matches!(outcome, DispatchOutcome::NoMatch { .. }));
    }

    #[test]
    fn args_after_trigger_are_captured() {
        let catalog = CatalogSpec::single(label_command("deploy", "/deploy"));
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("/deploy staging now", "alice"), &env).unwrap();
        let DispatchOutcome::Dispatched { args, .. } = outcome else {
            panic!("expected dispatch")
        };
        assert_eq!(args, vec!["staging".to_string(), "now".to_string()]);
    }

    #[test]
    fn comment_on_plain_issue_is_not_on_pull_request() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new();
        let outcome = apply(&catalog, &InboundEvent::IssueCommentNotOnPullRequest, &env).unwrap();
        assert_eq!(outcome, DispatchOutcome::NotOnPullRequest);
    }

    #[test]
    fn unresolvable_pull_request_is_rejected_not_a_hard_error() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new(); // no PR seeded for issue 7
        let outcome = apply(&catalog, &comment("/test", "alice"), &env).unwrap();
        assert!(matches!(outcome, DispatchOutcome::Rejected { .. }));
    }

    #[test]
    fn workflow_dispatch_target_substitutes_placeholders() {
        let mut inputs = std::collections::BTreeMap::new();
        inputs.insert("environment".to_string(), "$1".to_string());
        inputs.insert("sha".to_string(), "$checkout-ref".to_string());
        let cmd = CommentCommandSpec {
            name: "deploy".to_string(),
            trigger: "/deploy".to_string(),
            min_permission: Permission::Write,
            trust_pr_author: true,
            allowlist: Vec::new(),
            target: DispatchTarget::WorkflowDispatch {
                workflow: "deploy.yml".to_string(),
                git_ref: Some("$branch-ref".to_string()),
                inputs,
            },
        };
        let catalog = CatalogSpec::single(cmd);
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("/deploy staging", "alice"), &env).unwrap();
        assert!(outcome.dispatched());
        let dispatches = env.workflow_dispatches.borrow();
        let (workflow, git_ref, inputs) = &dispatches[0];
        assert_eq!(workflow, "deploy.yml");
        assert_eq!(git_ref, "refs/heads/feature-x");
        assert_eq!(
            inputs.get("environment").map(String::as_str),
            Some("staging")
        );
        assert_eq!(inputs.get("sha").map(String::as_str), Some("deadbeef"));
    }

    #[test]
    fn leading_whitespace_before_trigger_still_matches() {
        let catalog = CatalogSpec::single(label_command("test", "/test"));
        let env = MockEnvironment::new().with_pull_request(7, pr("alice"));
        let outcome = apply(&catalog, &comment("   /test", "alice"), &env).unwrap();
        assert!(outcome.dispatched());
    }
}
