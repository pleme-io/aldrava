//! The observation + mutation seam — [`Environment`]. A real impl
//! ([`GitHubEnvironment`]) talks to the GitHub REST API; the interpreter
//! never calls the network directly, so tests drive [`interp::apply`] against
//! an in-memory [`MockEnvironment`] with zero network access. "The trait IS
//! the testability contract" (TYPED-SPEC + INTERPRETER TRIPLET).

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde_json::Value;

use crate::spec::Permission;

/// A resolved pull request — the fields both trust resolution (`author`) and
/// dispatch-context resolution (`head_sha`/`head_ref`/`base_ref`) need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestInfo {
    pub author: String,
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvError {
    #[error("GitHub API request failed: {0}")]
    Request(String),
}

/// The observation + mutation seam. Every method that can fail returns a
/// typed [`EnvError`] rather than panicking; a "not found" case that the
/// original operation tolerates (e.g. removing a label that was never
/// present) is swallowed *inside* the implementation, not surfaced as an
/// error the interpreter must special-case.
pub trait Environment {
    /// The commenter's permission on the repo. An environment that cannot
    /// resolve this (network error, user has no collaborator record) returns
    /// [`Permission::None`] rather than erroring — never over-trusts on
    /// ambiguity.
    fn collaborator_permission(&self, login: &str) -> Permission;

    /// Fetch the pull request the comment/label event targets. `None` when
    /// the PR cannot be resolved.
    fn get_pull_request(&self, issue_number: u64) -> Option<PullRequestInfo>;

    fn add_label(&self, issue_number: u64, label: &str) -> Result<(), EnvError>;

    /// Remove `label` if present. Removing an absent label is success, not
    /// an error (mirrors GitHub's own idempotent-relabel pattern).
    fn remove_label(&self, issue_number: u64, label: &str) -> Result<(), EnvError>;

    fn dispatch_workflow(
        &self,
        workflow: &str,
        git_ref: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<(), EnvError>;

    fn repository_dispatch(
        &self,
        event_type: &str,
        repo: Option<&str>,
        payload: &Value,
    ) -> Result<(), EnvError>;

    /// Remove-then-add `label` — the idempotent relabel that re-fires a
    /// `pull_request: [labeled]`-gated pipeline even when the label is
    /// already present. A default method: every [`Environment`] gets it for
    /// free from `add_label`/`remove_label`.
    fn relabel(&self, issue_number: u64, label: &str) -> Result<(), EnvError> {
        self.remove_label(issue_number, label)?;
        self.add_label(issue_number, label)
    }
}

/// A recorded `dispatch_workflow` call.
pub type WorkflowDispatchCall = (String, String, BTreeMap<String, String>);
/// A recorded `repository_dispatch` call.
pub type RepositoryDispatchCall = (String, Option<String>, Value);

/// An in-memory [`Environment`] for tests. Records every mutation so a test
/// can assert exactly what would have happened, with zero network access.
#[derive(Debug, Default)]
pub struct MockEnvironment {
    pub permissions: BTreeMap<String, Permission>,
    pub pull_requests: BTreeMap<u64, PullRequestInfo>,
    pub labels_added: RefCell<Vec<(u64, String)>>,
    pub labels_removed: RefCell<Vec<(u64, String)>>,
    pub workflow_dispatches: RefCell<Vec<WorkflowDispatchCall>>,
    pub repository_dispatches: RefCell<Vec<RepositoryDispatchCall>>,
}

impl MockEnvironment {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_permission(mut self, login: impl Into<String>, permission: Permission) -> Self {
        self.permissions.insert(login.into(), permission);
        self
    }

    #[must_use]
    pub fn with_pull_request(mut self, issue_number: u64, info: PullRequestInfo) -> Self {
        self.pull_requests.insert(issue_number, info);
        self
    }
}

impl Environment for MockEnvironment {
    fn collaborator_permission(&self, login: &str) -> Permission {
        self.permissions.get(login).copied().unwrap_or(Permission::None)
    }

    fn get_pull_request(&self, issue_number: u64) -> Option<PullRequestInfo> {
        self.pull_requests.get(&issue_number).cloned()
    }

    fn add_label(&self, issue_number: u64, label: &str) -> Result<(), EnvError> {
        self.labels_added.borrow_mut().push((issue_number, label.to_string()));
        Ok(())
    }

    fn remove_label(&self, issue_number: u64, label: &str) -> Result<(), EnvError> {
        self.labels_removed.borrow_mut().push((issue_number, label.to_string()));
        Ok(())
    }

    fn dispatch_workflow(
        &self,
        workflow: &str,
        git_ref: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<(), EnvError> {
        self.workflow_dispatches
            .borrow_mut()
            .push((workflow.to_string(), git_ref.to_string(), inputs.clone()));
        Ok(())
    }

    fn repository_dispatch(
        &self,
        event_type: &str,
        repo: Option<&str>,
        payload: &Value,
    ) -> Result<(), EnvError> {
        self.repository_dispatches.borrow_mut().push((
            event_type.to_string(),
            repo.map(str::to_string),
            payload.clone(),
        ));
        Ok(())
    }
}

/// The real [`Environment`] — the GitHub REST API over `ureq`.
pub struct GitHubEnvironment {
    owner: String,
    repo: String,
    token: String,
    api_base: String,
    agent: ureq::Agent,
}

impl GitHubEnvironment {
    #[must_use]
    pub fn new(owner: impl Into<String>, repo: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            token: token.into(),
            api_base: std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_string()),
            agent: ureq::AgentBuilder::new().build(),
        }
    }

    fn repos_url(&self, owner: &str, repo: &str, suffix: &str) -> String {
        format!("{}/repos/{owner}/{repo}{suffix}", self.api_base)
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        self.agent
            .request(method, url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", "aldrava")
    }
}

impl Environment for GitHubEnvironment {
    fn collaborator_permission(&self, login: &str) -> Permission {
        let url = self.repos_url(&self.owner, &self.repo, &format!("/collaborators/{login}/permission"));
        match self.request("GET", &url).call() {
            Ok(resp) => resp
                .into_json::<Value>()
                .ok()
                .and_then(|v| v.get("permission").and_then(Value::as_str).map(Permission::from_github_str))
                .unwrap_or(Permission::None),
            Err(_) => Permission::None,
        }
    }

    fn get_pull_request(&self, issue_number: u64) -> Option<PullRequestInfo> {
        let url = self.repos_url(&self.owner, &self.repo, &format!("/pulls/{issue_number}"));
        let body: Value = self.request("GET", &url).call().ok()?.into_json().ok()?;
        Some(PullRequestInfo {
            author: body.get("user")?.get("login")?.as_str()?.to_string(),
            head_sha: body.get("head")?.get("sha")?.as_str()?.to_string(),
            head_ref: body.get("head")?.get("ref")?.as_str()?.to_string(),
            base_ref: body.get("base")?.get("ref")?.as_str()?.to_string(),
        })
    }

    fn add_label(&self, issue_number: u64, label: &str) -> Result<(), EnvError> {
        let url = self.repos_url(&self.owner, &self.repo, &format!("/issues/{issue_number}/labels"));
        self.request("POST", &url)
            .send_json(serde_json::json!({ "labels": [label] }))
            .map(|_| ())
            .map_err(|e| EnvError::Request(e.to_string()))
    }

    fn remove_label(&self, issue_number: u64, label: &str) -> Result<(), EnvError> {
        let encoded = urlencoding_light(label);
        let url = self.repos_url(&self.owner, &self.repo, &format!("/issues/{issue_number}/labels/{encoded}"));
        match self.request("DELETE", &url).call() {
            Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(EnvError::Request(e.to_string())),
        }
    }

    fn dispatch_workflow(
        &self,
        workflow: &str,
        git_ref: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<(), EnvError> {
        let url = self.repos_url(&self.owner, &self.repo, &format!("/actions/workflows/{workflow}/dispatches"));
        self.request("POST", &url)
            .send_json(serde_json::json!({ "ref": git_ref, "inputs": inputs }))
            .map(|_| ())
            .map_err(|e| EnvError::Request(e.to_string()))
    }

    fn repository_dispatch(
        &self,
        event_type: &str,
        repo: Option<&str>,
        payload: &Value,
    ) -> Result<(), EnvError> {
        let (owner, repo_name) = match repo.and_then(|r| r.split_once('/')) {
            Some((o, r)) => (o, r),
            None => (self.owner.as_str(), self.repo.as_str()),
        };
        let url = self.repos_url(owner, repo_name, "/dispatches");
        self.request("POST", &url)
            .send_json(serde_json::json!({ "event_type": event_type, "client_payload": payload }))
            .map(|_| ())
            .map_err(|e| EnvError::Request(e.to_string()))
    }
}

/// Percent-encode the handful of characters a GitHub label name can
/// plausibly contain that are meaningful in a URL path segment. Not a
/// general-purpose URL encoder — scoped to this one call site.
fn urlencoding_light(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Environment, MockEnvironment, PullRequestInfo};
    use crate::spec::Permission;

    #[test]
    fn mock_records_relabel_as_remove_then_add() {
        let env = MockEnvironment::new();
        env.relabel(7, "ci/run-tests").unwrap();
        assert_eq!(env.labels_removed.borrow().as_slice(), &[(7, "ci/run-tests".to_string())]);
        assert_eq!(env.labels_added.borrow().as_slice(), &[(7, "ci/run-tests".to_string())]);
    }

    #[test]
    fn mock_unknown_login_has_none_permission() {
        let env = MockEnvironment::new().with_permission("alice", Permission::Write);
        assert_eq!(env.collaborator_permission("alice"), Permission::Write);
        assert_eq!(env.collaborator_permission("mallory"), Permission::None);
    }

    #[test]
    fn mock_pull_request_lookup() {
        let env = MockEnvironment::new().with_pull_request(
            42,
            PullRequestInfo {
                author: "alice".into(),
                head_sha: "sha1".into(),
                head_ref: "feature".into(),
                base_ref: "develop".into(),
            },
        );
        assert_eq!(env.get_pull_request(42).unwrap().author, "alice");
        assert!(env.get_pull_request(99).is_none());
    }
}
