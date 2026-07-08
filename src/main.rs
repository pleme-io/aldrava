//! The `aldrava` CLI — the typed executable a GitHub Action's `run.tlisp`
//! wraps via `exec-capture` (no logic in shell; the shell layer only
//! installs the binary and captures its JSON stdout).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use aldrava::environment::GitHubEnvironment;
use aldrava::event::{self, RunContext};
use aldrava::interp::{self, DispatchOutcome};
use aldrava::spec::{CatalogSpec, CommentCommandSpec, DispatchTarget, Permission};
use aldrava::spec_lisp;

#[derive(Parser)]
#[command(
    name = "aldrava",
    version,
    about = "The typed knock — comment-command dispatch for GitHub Actions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve the inbound `issue_comment` event against a command catalog
    /// and — if a trusted knock matches — dispatch the target.
    Dispatch(Box<DispatchArgs>),
    /// Resolve a uniform run context from whatever event triggered this job
    /// (label add, `workflow_dispatch`, `schedule`, ...), with no command
    /// matching or trust decision involved. For the downstream pipeline a
    /// dispatched knock re-triggers.
    Resolve(ResolveArgs),
    /// Parse and validate a `.github/aldrava.lisp` catalog file, printing
    /// any syntax/semantic error with the offending form named.
    Lint(LintArgs),
}

#[derive(Parser)]
struct EventSource {
    /// Path to the GitHub Actions event payload JSON. Defaults to
    /// `$GITHUB_EVENT_PATH`.
    #[arg(long)]
    event_path: Option<PathBuf>,
    /// The event name. Defaults to `$GITHUB_EVENT_NAME`.
    #[arg(long)]
    event_name: Option<String>,
}

impl EventSource {
    fn resolve(&self) -> Result<(String, Value)> {
        let event_name = self
            .event_name
            .clone()
            .or_else(|| std::env::var("GITHUB_EVENT_NAME").ok())
            .context("event name not given: pass --event-name or set GITHUB_EVENT_NAME")?;
        let path = self
            .event_path
            .clone()
            .or_else(|| std::env::var("GITHUB_EVENT_PATH").ok().map(PathBuf::from))
            .context("event path not given: pass --event-path or set GITHUB_EVENT_PATH")?;
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading event payload at {}", path.display()))?;
        let payload: Value =
            serde_json::from_str(&raw).context("event payload is not valid JSON")?;
        Ok((event_name, payload))
    }
}

#[derive(Parser)]
struct RepoSource {
    /// `owner/repo`. Defaults to `$GITHUB_REPOSITORY`.
    #[arg(long)]
    repo: Option<String>,
    /// GitHub token. Defaults to `$GITHUB_TOKEN`.
    #[arg(long)]
    token: Option<String>,
}

impl RepoSource {
    fn resolve(&self) -> Result<(String, String, String)> {
        let repo_full = self
            .repo
            .clone()
            .or_else(|| std::env::var("GITHUB_REPOSITORY").ok())
            .context("repo not given: pass --repo or set GITHUB_REPOSITORY")?;
        let (owner, repo) = repo_full
            .split_once('/')
            .with_context(|| format!("--repo must be `owner/repo`, got `{repo_full}`"))?;
        let token = self
            .token
            .clone()
            .or_else(|| std::env::var("GITHUB_TOKEN").ok())
            .context("token not given: pass --token or set GITHUB_TOKEN")?;
        Ok((owner.to_string(), repo.to_string(), token))
    }
}

#[derive(Parser)]
struct DispatchArgs {
    #[command(flatten)]
    event: EventSource,
    #[command(flatten)]
    repo: RepoSource,

    /// Path to a `(defcommentcommand ...)` catalog file. Mutually exclusive
    /// with the inline `--command`/`--target-*` flags below.
    #[arg(long)]
    catalog: Option<PathBuf>,

    /// Inline single-command mode: the command's name (e.g. `test`). The
    /// trigger defaults to `/<command>` unless `--trigger` overrides it.
    #[arg(long)]
    command: Option<String>,
    #[arg(long)]
    trigger: Option<String>,
    #[arg(long, default_value = "write")]
    min_permission: String,
    #[arg(long, default_value_t = true)]
    trust_pr_author: bool,
    /// Comma-separated logins trusted regardless of permission.
    #[arg(long)]
    allowlist: Option<String>,
    #[arg(long)]
    target_label: Option<String>,
    #[arg(long)]
    target_workflow: Option<String>,
    #[arg(long)]
    target_workflow_ref: Option<String>,
    /// Repeatable `KEY=VALUE` `workflow_dispatch` input.
    #[arg(long = "input")]
    inputs: Vec<String>,
    #[arg(long)]
    target_repository_dispatch_event: Option<String>,
    #[arg(long)]
    target_repository_dispatch_repo: Option<String>,
}

impl DispatchArgs {
    fn resolve_catalog(&self) -> Result<CatalogSpec> {
        if let Some(path) = &self.catalog {
            let src = std::fs::read_to_string(path)
                .with_context(|| format!("reading catalog at {}", path.display()))?;
            return spec_lisp::parse(&src).map_err(|e| anyhow::anyhow!("{e}"));
        }
        let name = self
            .command
            .clone()
            .context("either --catalog or --command must be given")?;
        let trigger = self.trigger.clone().unwrap_or_else(|| format!("/{name}"));
        let min_permission = parse_permission(&self.min_permission)?;
        let allowlist = self
            .allowlist
            .as_deref()
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let target = self.resolve_target()?;
        Ok(CatalogSpec::single(CommentCommandSpec {
            name,
            trigger,
            min_permission,
            trust_pr_author: self.trust_pr_author,
            allowlist,
            target,
        }))
    }

    fn resolve_target(&self) -> Result<DispatchTarget> {
        if let Some(name) = &self.target_label {
            return Ok(DispatchTarget::Label { name: name.clone() });
        }
        if let Some(workflow) = &self.target_workflow {
            let mut inputs = BTreeMap::new();
            for kv in &self.inputs {
                let (k, v) = kv
                    .split_once('=')
                    .with_context(|| format!("--input must be KEY=VALUE, got `{kv}`"))?;
                inputs.insert(k.to_string(), v.to_string());
            }
            return Ok(DispatchTarget::WorkflowDispatch {
                workflow: workflow.clone(),
                git_ref: self.target_workflow_ref.clone(),
                inputs,
            });
        }
        if let Some(event_type) = &self.target_repository_dispatch_event {
            return Ok(DispatchTarget::RepositoryDispatch {
                event_type: event_type.clone(),
                repo: self.target_repository_dispatch_repo.clone(),
            });
        }
        bail!(
            "no target given: one of --target-label / --target-workflow / --target-repository-dispatch-event is required"
        )
    }
}

fn parse_permission(s: &str) -> Result<Permission> {
    Ok(match s {
        "none" => Permission::None,
        "read" => Permission::Read,
        "triage" => Permission::Triage,
        "write" => Permission::Write,
        "maintain" => Permission::Maintain,
        "admin" => Permission::Admin,
        other => bail!(
            "unknown --min-permission `{other}` — expected one of none|read|triage|write|maintain|admin"
        ),
    })
}

#[derive(Parser)]
struct ResolveArgs {
    #[command(flatten)]
    event: EventSource,
    /// Restrict a `pull_request: [labeled]` event to this label. Omit to
    /// treat every `pull_request` action as eligible.
    #[arg(long)]
    label_name: Option<String>,
}

#[derive(Parser)]
struct LintArgs {
    catalog: PathBuf,
}

fn outcome_to_json(outcome: &DispatchOutcome) -> Value {
    match outcome {
        DispatchOutcome::NotOnPullRequest => json!({
            "dispatched": false,
            "outcome": "not-on-pull-request",
        }),
        DispatchOutcome::NoMatch { reason } => json!({
            "dispatched": false,
            "outcome": "no-match",
            "reason": reason,
        }),
        DispatchOutcome::Rejected {
            command,
            commenter,
            reason,
        } => json!({
            "dispatched": false,
            "outcome": "rejected",
            "command": command,
            "commenter": commenter,
            "reason": reason,
        }),
        DispatchOutcome::Dispatched {
            command,
            args,
            commenter,
            context,
            target_kind,
            target_detail,
        } => json!({
            "dispatched": true,
            "outcome": "dispatched",
            "command": command,
            "args": args,
            "commenter": commenter,
            "checkout_ref": context.checkout_ref,
            "branch_ref": context.branch_ref,
            "base_ref": context.base_ref,
            "pr_author": context.pr_author,
            "is_develop": context.is_develop,
            "target_kind": target_kind,
            "target_detail": target_detail,
        }),
    }
}

fn run_dispatch(args: &DispatchArgs) -> Result<Value> {
    let catalog = args.resolve_catalog()?;
    let (event_name, payload) = args.event.resolve()?;
    let (owner, repo, token) = args.repo.resolve()?;
    let inbound = event::resolve(&event_name, &payload);
    let env = GitHubEnvironment::new(owner, repo, token);
    let outcome = interp::apply(&catalog, &inbound, &env).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(outcome_to_json(&outcome))
}

fn run_resolve(args: &ResolveArgs) -> Result<Value> {
    let (event_name, payload) = args.event.resolve()?;
    let inbound = event::resolve(&event_name, &payload);
    let fallback_sha = std::env::var("GITHUB_SHA").unwrap_or_default();
    let fallback_ref = std::env::var("GITHUB_REF").unwrap_or_default();
    let ctx = RunContext::resolve(
        &inbound,
        args.label_name.as_deref(),
        &fallback_sha,
        &fallback_ref,
    );
    Ok(serde_json::to_value(ctx)?)
}

fn run_lint(args: &LintArgs) -> Result<Value> {
    let src = std::fs::read_to_string(&args.catalog)
        .with_context(|| format!("reading catalog at {}", args.catalog.display()))?;
    let catalog = spec_lisp::parse(&src).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(json!({
        "ok": true,
        "commands": catalog.commands.iter().map(|c| &c.name).collect::<Vec<_>>(),
    }))
}

fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Dispatch(args) => run_dispatch(args),
        Command::Resolve(args) => run_resolve(args),
        Command::Lint(args) => run_lint(args),
    };
    match result {
        Ok(value) => println!("{value}"),
        Err(err) => {
            eprintln!("{}", json!({ "ok": false, "error": err.to_string() }));
            std::process::exit(1);
        }
    }
}
