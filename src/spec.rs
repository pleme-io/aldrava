//! The typed border — the `(defcommentcommand ...)` catalog shape.
//!
//! Part 1 of the TYPED-SPEC + INTERPRETER TRIPLET (`pleme-io/CLAUDE.md`): a
//! consuming repo declares its comment-triggered commands as a
//! [`CommentCommandSpec`] table (authored either as a `(defcommentcommand
//! ...)` Lisp form via [`crate::spec_lisp`], or inline from CLI flags for the
//! single-command degenerate case). `#[serde(deny_unknown_fields)]` makes a
//! typo'd field a parse error rather than a silently-ignored one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A GitHub repository permission tier, ordered from least to most
/// privileged. Declaration order IS the trust order — `Ord`/`PartialOrd`
/// derive from it, so `commenter >= spec.min_permission` is a typed
/// comparison, never a hand-rolled string match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    None,
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

impl Permission {
    /// Parse a GitHub REST `permission` string
    /// (`GET /repos/{owner}/{repo}/collaborators/{username}/permission`).
    /// An unrecognized value maps to [`Permission::None`] — never panics,
    /// never silently upgrades trust on an unexpected API shape.
    #[must_use]
    pub fn from_github_str(s: &str) -> Self {
        match s {
            "read" => Self::Read,
            "triage" => Self::Triage,
            "write" => Self::Write,
            "maintain" => Self::Maintain,
            "admin" => Self::Admin,
            _ => Self::None,
        }
    }
}

fn default_min_permission() -> Permission {
    Permission::Write
}

const fn default_true() -> bool {
    true
}

/// Where a trusted knock is dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum DispatchTarget {
    /// Remove-then-add a label on the PR/issue — the idempotent relabel
    /// pattern that re-fires a `pull_request: [labeled]`-gated pipeline even
    /// when the label is already present.
    Label { name: String },
    /// Fire a `workflow_dispatch` against a workflow file on the resolved
    /// branch ref. Input values may reference `$checkout-ref`, `$branch-ref`,
    /// `$base-ref`, `$pr-author`, or a positional capture `$1`.."$9" —
    /// substituted by the interpreter from the resolved [`DispatchContext`]
    /// and the command's captured args.
    WorkflowDispatch {
        workflow: String,
        #[serde(rename = "ref", default)]
        git_ref: Option<String>,
        #[serde(default)]
        inputs: BTreeMap<String, String>,
    },
    /// Fire a `repository_dispatch` event, optionally against another repo
    /// (`owner/repo`); defaults to the current repo.
    RepositoryDispatch {
        event_type: String,
        #[serde(default)]
        repo: Option<String>,
    },
}

/// One registered `/command` — the unit a consuming repo authors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentCommandSpec {
    /// The command's name, for logging/output only (`"test"`, `"deploy"`).
    pub name: String,
    /// The literal prefix a comment must start with, e.g. `"/test"`. Matched
    /// as `trigger` followed by whitespace-or-end-of-string — `"/test"`
    /// matches `/test` and `/test staging` but not `/testing`.
    pub trigger: String,
    /// The minimum GitHub repo permission a commenter must hold. Ignored for
    /// the PR author when `trust_pr_author` is true, and for any login in
    /// `allowlist`.
    #[serde(default = "default_min_permission")]
    pub min_permission: Permission,
    /// When true (default), the PR's own author is always trusted for this
    /// command regardless of `min_permission`.
    #[serde(default = "default_true")]
    pub trust_pr_author: bool,
    /// Logins trusted for this command regardless of `min_permission` or PR
    /// authorship — e.g. a bot account with no repo collaborator record.
    #[serde(default)]
    pub allowlist: Vec<String>,
    /// What happens when a trusted knock matches.
    pub target: DispatchTarget,
}

/// The full registered-command table for a repo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSpec {
    pub commands: Vec<CommentCommandSpec>,
}

impl CatalogSpec {
    /// Build a single-command catalog inline — the degenerate case a
    /// consumer reaches via plain CLI flags / Action inputs with no
    /// `(defcommentcommand ...)` file to author.
    #[must_use]
    pub fn single(command: CommentCommandSpec) -> Self {
        Self {
            commands: vec![command],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Permission, Permission::*};

    #[test]
    fn permission_ordering_matches_github_privilege_order() {
        let ascending = [None, Read, Triage, Write, Maintain, Admin];
        for pair in ascending.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?} must be strictly ascending");
        }
    }

    #[test]
    fn from_github_str_unknown_value_is_none() {
        assert_eq!(Permission::from_github_str("bogus"), Permission::None);
        assert_eq!(Permission::from_github_str(""), Permission::None);
    }

    #[test]
    fn from_github_str_known_values() {
        assert_eq!(Permission::from_github_str("write"), Permission::Write);
        assert_eq!(Permission::from_github_str("admin"), Permission::Admin);
    }
}
