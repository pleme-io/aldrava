//! The inbound-event border — the subset of a GitHub Actions webhook payload
//! `aldrava` actually reads, resolved into one of three typed shapes
//! regardless of which trigger fired the job.
//!
//! This generalizes the "resolve whatever triggered this run into one
//! uniform context" shape common to comment-triggered CI dispatch: a
//! downstream pipeline may be re-triggered by a label add, a direct
//! `workflow_dispatch`, a `schedule`, or a `repository_dispatch` — and every
//! one of those needs the same `(checkout_ref, branch_ref, base_ref,
//! pr_author, is_develop)` context, resolved uniformly. Deliberately does
//! NOT attempt to model the full GitHub webhook schema (that is upstream's
//! job, not ours) — only the handful of JSON pointers each shape needs are
//! read, defensively, with `serde_json::Value` as the escape hatch.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One resolved inbound trigger, uniform across event sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundEvent {
    /// `issue_comment: created` on a pull request. The only shape a knock
    /// (comment command) can arrive on.
    IssueComment {
        issue_number: u64,
        comment_body: String,
        commenter_login: String,
    },
    /// `issue_comment` on a plain issue (no `pull_request` field) — never a
    /// knock target; carried as its own variant so callers get an explicit,
    /// named reason rather than inferring "not a PR" from absence.
    IssueCommentNotOnPullRequest,
    /// `pull_request: [labeled]` — used by a downstream pipeline to resolve
    /// its own run context uniformly when re-triggered by a relabel.
    PullRequestLabeled {
        label_name: String,
        head_sha: String,
        head_ref: String,
        base_ref: String,
        pr_author: String,
    },
    /// `pull_request` with any other action — context resolvable, but no
    /// label match to gate on.
    PullRequestOther {
        head_sha: String,
        head_ref: String,
        base_ref: String,
        pr_author: String,
    },
    /// `workflow_dispatch` / `repository_dispatch` / `schedule` /
    /// `workflow_call` — always eligible to run; no comment/label context.
    AlwaysRun { event_name: String },
    /// An event source `aldrava` has no typed handling for.
    Unsupported { event_name: String },
}

/// `"feature-x"` -> `"refs/heads/feature-x"`. Shared by [`RunContext`] and the
/// interpreter so the two never drift on how a ref is normalized.
#[must_use]
pub fn to_branch_ref(short_ref: &str) -> String {
    format!("refs/heads/{short_ref}")
}

/// Whether a (already-normalized) `refs/heads/...` ref is `develop`.
#[must_use]
pub fn is_develop_ref(branch_ref: &str) -> bool {
    branch_ref == "refs/heads/develop"
}

fn str_field<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

/// Resolve the raw `(event_name, payload)` pair GitHub Actions hands every
/// job (`$GITHUB_EVENT_NAME`, `$GITHUB_EVENT_PATH`) into one
/// [`InboundEvent`]. Never panics on a missing/malformed field — a payload
/// this function cannot make sense of resolves to [`InboundEvent::Unsupported`]
/// rather than erroring, so callers always get an answer to branch on.
#[must_use]
pub fn resolve(event_name: &str, payload: &Value) -> InboundEvent {
    match event_name {
        "issue_comment" => {
            let is_pr = payload
                .get("issue")
                .and_then(|i| i.get("pull_request"))
                .is_some();
            if !is_pr {
                return InboundEvent::IssueCommentNotOnPullRequest;
            }
            let issue_number = payload
                .get("issue")
                .and_then(|i| i.get("number"))
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let comment_body = str_field(payload, &["comment", "body"])
                .unwrap_or_default()
                .to_string();
            let commenter_login = str_field(payload, &["comment", "user", "login"])
                .unwrap_or_default()
                .to_string();
            InboundEvent::IssueComment {
                issue_number,
                comment_body,
                commenter_login,
            }
        }
        "pull_request" | "pull_request_target" => {
            let head_sha = str_field(payload, &["pull_request", "head", "sha"])
                .unwrap_or_default()
                .to_string();
            let head_ref = str_field(payload, &["pull_request", "head", "ref"])
                .unwrap_or_default()
                .to_string();
            let base_ref = str_field(payload, &["pull_request", "base", "ref"])
                .unwrap_or_default()
                .to_string();
            let pr_author = str_field(payload, &["pull_request", "user", "login"])
                .unwrap_or_default()
                .to_string();
            match str_field(payload, &["action"]) {
                Some("labeled") => {
                    let label_name = str_field(payload, &["label", "name"])
                        .unwrap_or_default()
                        .to_string();
                    InboundEvent::PullRequestLabeled {
                        label_name,
                        head_sha,
                        head_ref,
                        base_ref,
                        pr_author,
                    }
                }
                _ => InboundEvent::PullRequestOther {
                    head_sha,
                    head_ref,
                    base_ref,
                    pr_author,
                },
            }
        }
        "workflow_dispatch" | "repository_dispatch" | "schedule" | "workflow_call" => {
            InboundEvent::AlwaysRun {
                event_name: event_name.to_string(),
            }
        }
        other => InboundEvent::Unsupported {
            event_name: other.to_string(),
        },
    }
}

/// The uniform context a downstream pipeline resolves regardless of trigger
/// source. Mirrors the akeyless-style `(should_run, checkout_ref, branch_ref,
/// base_ref, pr_author, is_develop)` output contract, generalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunContext {
    pub should_run: bool,
    pub checkout_ref: String,
    pub branch_ref: String,
    pub base_ref: String,
    pub pr_author: String,
    pub is_develop: bool,
    pub reason: String,
}

impl RunContext {
    #[must_use]
    pub fn not_running(reason: impl Into<String>) -> Self {
        Self {
            should_run: false,
            checkout_ref: String::new(),
            branch_ref: String::new(),
            base_ref: String::new(),
            pr_author: String::new(),
            is_develop: false,
            reason: reason.into(),
        }
    }

    #[must_use]
    fn from_pr_fields(head_sha: &str, head_ref: &str, base_ref: &str, pr_author: &str) -> Self {
        let branch_ref = to_branch_ref(head_ref);
        let base = to_branch_ref(base_ref);
        Self {
            should_run: true,
            checkout_ref: head_sha.to_string(),
            branch_ref: branch_ref.clone(),
            base_ref: base,
            pr_author: pr_author.to_string(),
            is_develop: is_develop_ref(&branch_ref),
            reason: String::from("resolved"),
        }
    }

    /// Resolve a [`RunContext`] for the "downstream pipeline" use of the
    /// resolver: given the *default branch's* fallback ref (used when the
    /// event carries no PR at all, e.g. a push-triggered `schedule` run),
    /// decide whether this run should proceed and with what context.
    ///
    /// `wanted_label` is `None` for a pipeline that runs on every
    /// `pull_request` action / always-run trigger; `Some(name)` restricts a
    /// `pull_request: [labeled]` event to that specific label.
    #[must_use]
    pub fn resolve(
        event: &InboundEvent,
        wanted_label: Option<&str>,
        fallback_sha: &str,
        fallback_ref: &str,
    ) -> Self {
        match event {
            InboundEvent::PullRequestLabeled {
                label_name,
                head_sha,
                head_ref,
                base_ref,
                pr_author,
            } => {
                if wanted_label.is_some_and(|w| w != label_name) {
                    return Self::not_running(format!(
                        "label `{label_name}` does not match expected `{}`",
                        wanted_label.unwrap_or_default()
                    ));
                }
                Self::from_pr_fields(head_sha, head_ref, base_ref, pr_author)
            }
            InboundEvent::PullRequestOther {
                head_sha,
                head_ref,
                base_ref,
                pr_author,
            } if wanted_label.is_none() => {
                Self::from_pr_fields(head_sha, head_ref, base_ref, pr_author)
            }
            InboundEvent::AlwaysRun { .. } => {
                let branch_ref = fallback_ref.to_string();
                let is_develop = is_develop_ref(&branch_ref);
                Self {
                    should_run: true,
                    checkout_ref: fallback_sha.to_string(),
                    branch_ref,
                    base_ref: String::new(),
                    pr_author: String::new(),
                    is_develop,
                    reason: String::from("always-run event"),
                }
            }
            InboundEvent::PullRequestOther { .. } => {
                Self::not_running("pull_request action does not carry the requested label")
            }
            InboundEvent::IssueComment { .. }
            | InboundEvent::IssueCommentNotOnPullRequest
            | InboundEvent::Unsupported { .. } => {
                Self::not_running("event source has no resolvable run context")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InboundEvent, RunContext, resolve};
    use serde_json::json;

    #[test]
    fn issue_comment_on_pr_resolves_with_body_and_author() {
        let payload = json!({
            "issue": {"number": 42, "pull_request": {"url": "x"}},
            "comment": {"body": "/test", "user": {"login": "alice"}}
        });
        assert_eq!(
            resolve("issue_comment", &payload),
            InboundEvent::IssueComment {
                issue_number: 42,
                comment_body: "/test".into(),
                commenter_login: "alice".into(),
            }
        );
    }

    #[test]
    fn issue_comment_on_plain_issue_is_not_a_pr() {
        let payload = json!({"issue": {"number": 1}, "comment": {"body": "/test"}});
        assert_eq!(
            resolve("issue_comment", &payload),
            InboundEvent::IssueCommentNotOnPullRequest
        );
    }

    #[test]
    fn pull_request_labeled_carries_full_context() {
        let payload = json!({
            "action": "labeled",
            "label": {"name": "ci/run-tests"},
            "pull_request": {
                "head": {"sha": "abc123", "ref": "feature-x"},
                "base": {"ref": "develop"},
                "user": {"login": "alice"}
            }
        });
        let ev = resolve("pull_request", &payload);
        assert_eq!(
            ev,
            InboundEvent::PullRequestLabeled {
                label_name: "ci/run-tests".into(),
                head_sha: "abc123".into(),
                head_ref: "feature-x".into(),
                base_ref: "develop".into(),
                pr_author: "alice".into(),
            }
        );
        let ctx = RunContext::resolve(&ev, Some("ci/run-tests"), "", "");
        assert!(ctx.should_run);
        // is_develop tracks the PR's own HEAD ref ("feature-x"), not its base.
        assert!(!ctx.is_develop);
        assert_eq!(ctx.checkout_ref, "abc123");
    }

    #[test]
    fn pull_request_labeled_head_ref_develop_is_develop_true() {
        let payload = json!({
            "action": "labeled",
            "label": {"name": "ci/run-tests"},
            "pull_request": {
                "head": {"sha": "abc123", "ref": "develop"},
                "base": {"ref": "main"},
                "user": {"login": "alice"}
            }
        });
        let ev = resolve("pull_request", &payload);
        let ctx = RunContext::resolve(&ev, Some("ci/run-tests"), "", "");
        assert!(ctx.is_develop);
    }

    #[test]
    fn pull_request_labeled_wrong_label_does_not_run() {
        let payload = json!({
            "action": "labeled",
            "label": {"name": "other-label"},
            "pull_request": {
                "head": {"sha": "abc123", "ref": "feature-x"},
                "base": {"ref": "main"},
                "user": {"login": "alice"}
            }
        });
        let ev = resolve("pull_request", &payload);
        let ctx = RunContext::resolve(&ev, Some("ci/run-tests"), "", "");
        assert!(!ctx.should_run);
    }

    #[test]
    fn always_run_events_resolve_should_run_true() {
        for name in ["workflow_dispatch", "repository_dispatch", "schedule", "workflow_call"] {
            let ev = resolve(name, &json!({}));
            assert_eq!(ev, InboundEvent::AlwaysRun { event_name: name.into() });
            let ctx = RunContext::resolve(&ev, Some("ci/run-tests"), "deadbeef", "refs/heads/main");
            assert!(ctx.should_run);
            assert_eq!(ctx.checkout_ref, "deadbeef");
        }
    }

    #[test]
    fn unsupported_event_name_is_named_not_panicking() {
        assert_eq!(
            resolve("pull_request_review", &json!({})),
            InboundEvent::Unsupported { event_name: "pull_request_review".into() }
        );
    }
}
