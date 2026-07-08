//! The Lisp leg of the TYPED-SPEC + INTERPRETER TRIPLET — a real, working
//! parser for the `(defcommentcommand ...)` authoring form, not documentation
//! parity. A consuming repo's `.github/aldrava.lisp` is canonical DATA:
//!
//! ```lisp
//! (defcommentcommand "test"
//!   :trigger "/test"
//!   :min-permission write
//!   :trust-pr-author true
//!   :target (label "ci/run-tests"))
//!
//! (defcommentcommand "deploy"
//!   :trigger "/deploy"
//!   :min-permission admin
//!   :trust-pr-author false
//!   :allowlist ("release-bot")
//!   :target (workflow-dispatch "deploy.yml"
//!             :ref "$branch-ref"
//!             :inputs ((environment "$1"))))
//! ```
//!
//! Tier-honest note (never round up): this is `aldrava`'s own minimal
//! recursive-descent S-expression reader for its one grammar, not an
//! invocation of the shared `tatara_lisp` crate's `#[derive(TataraDomain)]`
//! registration machinery — that crate is not yet a runtime-consumable
//! parsing library for external crates as of this writing (verified against
//! the two most recent typed-spec-triplet references in the fleet, neither
//! of which depends on it either). Swapping this reader for
//! `tatara_lisp::domain::register::<CatalogSpec>()` once that surface ships
//! is a named, isolated follow-up — the keyword vocabulary here already
//! matches the fleet's `(def<thing> "name" :key value ...)` convention, so
//! the swap changes only this file.

use std::collections::BTreeMap;
use std::fmt;

use crate::spec::{CatalogSpec, CommentCommandSpec, DispatchTarget, Permission};

#[derive(Debug, Clone, PartialEq, Eq)]
enum SExpr {
    Sym(String),
    Str(String),
    List(Vec<SExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LintError {
    #[error("{0}")]
    Syntax(String),
    #[error("in `{form}`: {message}")]
    Semantic { form: String, message: String },
}

impl fmt::Display for SExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SExpr::Sym(s) => write!(f, "{s}"),
            SExpr::Str(s) => write!(f, "{s:?}"),
            SExpr::List(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, ")")
            }
        }
    }
}

struct Tokenizer<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    src: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    Sym(String),
    Str(String),
}

impl<'a> Tokenizer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.char_indices().peekable(),
            src,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, LintError> {
        let mut out = Vec::new();
        while let Some(&(i, c)) = self.chars.peek() {
            match c {
                c if c.is_whitespace() => {
                    self.chars.next();
                }
                ';' => {
                    while let Some(&(_, c)) = self.chars.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.chars.next();
                    }
                }
                '(' => {
                    self.chars.next();
                    out.push(Token::Open);
                }
                ')' => {
                    self.chars.next();
                    out.push(Token::Close);
                }
                '"' => {
                    self.chars.next();
                    let mut s = String::new();
                    loop {
                        match self.chars.next() {
                            Some((_, '"')) => break,
                            Some((_, '\\')) => match self.chars.next() {
                                Some((_, 'n')) => s.push('\n'),
                                Some((_, '"')) => s.push('"'),
                                Some((_, '\\')) => s.push('\\'),
                                Some((_, other)) => s.push(other),
                                None => {
                                    return Err(LintError::Syntax(
                                        "unterminated string escape at end of input".into(),
                                    ));
                                }
                            },
                            Some((_, c)) => s.push(c),
                            None => {
                                return Err(LintError::Syntax(format!(
                                    "unterminated string starting near byte {i}"
                                )));
                            }
                        }
                    }
                    out.push(Token::Str(s));
                }
                _ => {
                    let start = i;
                    let mut end = i;
                    while let Some(&(j, c)) = self.chars.peek() {
                        if c.is_whitespace() || c == '(' || c == ')' || c == ';' {
                            break;
                        }
                        end = j + c.len_utf8();
                        self.chars.next();
                    }
                    out.push(Token::Sym(self.src[start..end].to_string()));
                }
            }
        }
        Ok(out)
    }
}

fn parse_all(tokens: &[Token]) -> Result<Vec<SExpr>, LintError> {
    let mut pos = 0;
    let mut forms = Vec::new();
    while pos < tokens.len() {
        let (expr, next) = parse_one(tokens, pos)?;
        forms.push(expr);
        pos = next;
    }
    Ok(forms)
}

fn parse_one(tokens: &[Token], pos: usize) -> Result<(SExpr, usize), LintError> {
    match tokens.get(pos) {
        Some(Token::Open) => {
            let mut items = Vec::new();
            let mut cur = pos + 1;
            loop {
                match tokens.get(cur) {
                    Some(Token::Close) => return Ok((SExpr::List(items), cur + 1)),
                    Some(_) => {
                        let (expr, next) = parse_one(tokens, cur)?;
                        items.push(expr);
                        cur = next;
                    }
                    None => return Err(LintError::Syntax("unexpected end of input inside `(...)`; missing `)`".into())),
                }
            }
        }
        Some(Token::Close) => Err(LintError::Syntax("unexpected `)` with no matching `(`".into())),
        Some(Token::Sym(s)) => Ok((SExpr::Sym(s.clone()), pos + 1)),
        Some(Token::Str(s)) => Ok((SExpr::Str(s.clone()), pos + 1)),
        None => Err(LintError::Syntax("unexpected end of input".into())),
    }
}

/// Split a form's argument list into `(positional, keyword-pairs)` — the
/// `(head arg1 arg2 :key1 val1 :key2 val2 ...)` shape every fleet `(def...)`
/// form uses. Keys must be `SExpr::Sym` starting with `:`.
fn split_kwargs(items: &[SExpr]) -> Result<(Vec<SExpr>, BTreeMap<String, SExpr>), LintError> {
    let mut positional = Vec::new();
    let mut kwargs = BTreeMap::new();
    let mut i = 0;
    while i < items.len() {
        if let SExpr::Sym(s) = &items[i]
            && let Some(key) = s.strip_prefix(':')
        {
            let val = items.get(i + 1).cloned().ok_or_else(|| LintError::Syntax(format!("keyword `:{key}` has no value")))?;
            kwargs.insert(key.to_string(), val);
            i += 2;
            continue;
        }
        positional.push(items[i].clone());
        i += 1;
    }
    Ok((positional, kwargs))
}

fn expect_str(expr: &SExpr, ctx: &str) -> Result<String, LintError> {
    match expr {
        SExpr::Str(s) => Ok(s.clone()),
        other => Err(LintError::Semantic {
            form: ctx.to_string(),
            message: format!("expected a string, got `{other}`"),
        }),
    }
}

fn expect_sym(expr: &SExpr, ctx: &str) -> Result<String, LintError> {
    match expr {
        SExpr::Sym(s) => Ok(s.clone()),
        other => Err(LintError::Semantic {
            form: ctx.to_string(),
            message: format!("expected a bare symbol, got `{other}`"),
        }),
    }
}

fn expect_list<'a>(expr: &'a SExpr, ctx: &str) -> Result<&'a [SExpr], LintError> {
    match expr {
        SExpr::List(items) => Ok(items),
        other => Err(LintError::Semantic {
            form: ctx.to_string(),
            message: format!("expected a list `(...)`, got `{other}`"),
        }),
    }
}

fn parse_bool(expr: &SExpr, ctx: &str) -> Result<bool, LintError> {
    match expect_sym(expr, ctx)?.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(LintError::Semantic {
            form: ctx.to_string(),
            message: format!("expected `true` or `false`, got `{other}`"),
        }),
    }
}

fn parse_permission(expr: &SExpr, ctx: &str) -> Result<Permission, LintError> {
    match expect_sym(expr, ctx)?.as_str() {
        "none" => Ok(Permission::None),
        "read" => Ok(Permission::Read),
        "triage" => Ok(Permission::Triage),
        "write" => Ok(Permission::Write),
        "maintain" => Ok(Permission::Maintain),
        "admin" => Ok(Permission::Admin),
        other => Err(LintError::Semantic {
            form: ctx.to_string(),
            message: format!(
                "unknown :min-permission `{other}` — expected one of none|read|triage|write|maintain|admin"
            ),
        }),
    }
}

fn parse_string_list(expr: &SExpr, ctx: &str) -> Result<Vec<String>, LintError> {
    expect_list(expr, ctx)?
        .iter()
        .map(|e| expect_str(e, ctx))
        .collect()
}

fn parse_inputs(expr: &SExpr, ctx: &str) -> Result<BTreeMap<String, String>, LintError> {
    let mut map = BTreeMap::new();
    for pair in expect_list(expr, ctx)? {
        let pair_items = expect_list(pair, ctx)?;
        if pair_items.len() != 2 {
            return Err(LintError::Semantic {
                form: ctx.to_string(),
                message: format!("each :inputs entry must be `(key \"value\")`, got `{pair}`"),
            });
        }
        let key = expect_sym(&pair_items[0], ctx)?;
        let val = expect_str(&pair_items[1], ctx)?;
        map.insert(key, val);
    }
    Ok(map)
}

fn parse_target(expr: &SExpr, ctx: &str) -> Result<DispatchTarget, LintError> {
    let items = expect_list(expr, ctx)?;
    let (positional, kwargs) = split_kwargs(items)?;
    let head = positional
        .first()
        .ok_or_else(|| LintError::Semantic {
            form: ctx.to_string(),
            message: "empty :target form".to_string(),
        })
        .and_then(|e| expect_sym(e, ctx))?;
    match head.as_str() {
        "label" => {
            let name = positional
                .get(1)
                .ok_or_else(|| LintError::Semantic {
                    form: ctx.to_string(),
                    message: "`(label ...)` requires a label-name string".to_string(),
                })
                .and_then(|e| expect_str(e, ctx))?;
            Ok(DispatchTarget::Label { name })
        }
        "workflow-dispatch" => {
            let workflow = positional
                .get(1)
                .ok_or_else(|| LintError::Semantic {
                    form: ctx.to_string(),
                    message: "`(workflow-dispatch ...)` requires a workflow-file string".to_string(),
                })
                .and_then(|e| expect_str(e, ctx))?;
            let git_ref = kwargs.get("ref").map(|e| expect_str(e, ctx)).transpose()?;
            let inputs = kwargs
                .get("inputs")
                .map(|e| parse_inputs(e, ctx))
                .transpose()?
                .unwrap_or_default();
            Ok(DispatchTarget::WorkflowDispatch {
                workflow,
                git_ref,
                inputs,
            })
        }
        "repository-dispatch" => {
            let event_type = positional
                .get(1)
                .ok_or_else(|| LintError::Semantic {
                    form: ctx.to_string(),
                    message: "`(repository-dispatch ...)` requires an event-type string".to_string(),
                })
                .and_then(|e| expect_str(e, ctx))?;
            let repo = kwargs.get("repo").map(|e| expect_str(e, ctx)).transpose()?;
            Ok(DispatchTarget::RepositoryDispatch { event_type, repo })
        }
        other => Err(LintError::Semantic {
            form: ctx.to_string(),
            message: format!(
                "unknown :target kind `{other}` — expected one of label|workflow-dispatch|repository-dispatch"
            ),
        }),
    }
}

fn parse_defcommentcommand(items: &[SExpr]) -> Result<CommentCommandSpec, LintError> {
    let (positional, kwargs) = split_kwargs(items)?;
    let name = positional
        .first()
        .ok_or_else(|| LintError::Semantic {
            form: "defcommentcommand".to_string(),
            message: "missing the command-name string".to_string(),
        })
        .and_then(|e| expect_str(e, "defcommentcommand"))?;
    let ctx = format!("defcommentcommand \"{name}\"");

    let trigger = kwargs
        .get("trigger")
        .ok_or_else(|| LintError::Semantic {
            form: ctx.clone(),
            message: "missing required `:trigger`".to_string(),
        })
        .and_then(|e| expect_str(e, &ctx))?;
    let min_permission = kwargs
        .get("min-permission")
        .map(|e| parse_permission(e, &ctx))
        .transpose()?
        .unwrap_or(Permission::Write);
    let trust_pr_author = kwargs
        .get("trust-pr-author")
        .map(|e| parse_bool(e, &ctx))
        .transpose()?
        .unwrap_or(true);
    let allowlist = kwargs
        .get("allowlist")
        .map(|e| parse_string_list(e, &ctx))
        .transpose()?
        .unwrap_or_default();
    let target = kwargs
        .get("target")
        .ok_or_else(|| LintError::Semantic {
            form: ctx.clone(),
            message: "missing required `:target`".to_string(),
        })
        .and_then(|e| parse_target(e, &ctx))?;

    Ok(CommentCommandSpec {
        name,
        trigger,
        min_permission,
        trust_pr_author,
        allowlist,
        target,
    })
}

/// Parse a full `.lisp` catalog source into a [`CatalogSpec`]. Every
/// top-level form must be `(defcommentcommand ...)` — anything else is a
/// [`LintError`] naming the offending form, never a silent skip.
pub fn parse(src: &str) -> Result<CatalogSpec, LintError> {
    let tokens = Tokenizer::new(src).tokenize()?;
    let forms = parse_all(&tokens)?;
    let mut commands = Vec::with_capacity(forms.len());
    for form in &forms {
        let SExpr::List(items) = form else {
            return Err(LintError::Syntax(format!(
                "top-level form must be `(defcommentcommand ...)`, got `{form}`"
            )));
        };
        let Some(SExpr::Sym(head)) = items.first() else {
            return Err(LintError::Syntax(
                "top-level form must start with a symbol".to_string(),
            ));
        };
        if head != "defcommentcommand" {
            return Err(LintError::Syntax(format!(
                "unknown top-level form `{head}` — only `defcommentcommand` is recognized"
            )));
        }
        commands.push(parse_defcommentcommand(&items[1..])?);
    }
    Ok(CatalogSpec { commands })
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::spec::{DispatchTarget, Permission};

    #[test]
    fn parses_the_minimal_label_form() {
        let src = r#"
            (defcommentcommand "test"
              :trigger "/test"
              :min-permission write
              :trust-pr-author true
              :target (label "ci/run-tests"))
        "#;
        let catalog = parse(src).expect("parses");
        assert_eq!(catalog.commands.len(), 1);
        let cmd = &catalog.commands[0];
        assert_eq!(cmd.name, "test");
        assert_eq!(cmd.trigger, "/test");
        assert_eq!(cmd.min_permission, Permission::Write);
        assert!(cmd.trust_pr_author);
        assert_eq!(cmd.target, DispatchTarget::Label { name: "ci/run-tests".into() });
    }

    #[test]
    fn parses_workflow_dispatch_with_inputs_and_allowlist() {
        let src = r#"
            (defcommentcommand "deploy"
              :trigger "/deploy"
              :min-permission admin
              :trust-pr-author false
              :allowlist ("release-bot" "ops-team-bot")
              :target (workflow-dispatch "deploy.yml"
                        :ref "$branch-ref"
                        :inputs ((environment "$1"))))
        "#;
        let catalog = parse(src).expect("parses");
        let cmd = &catalog.commands[0];
        assert_eq!(cmd.min_permission, Permission::Admin);
        assert!(!cmd.trust_pr_author);
        assert_eq!(cmd.allowlist, vec!["release-bot", "ops-team-bot"]);
        match &cmd.target {
            DispatchTarget::WorkflowDispatch { workflow, git_ref, inputs } => {
                assert_eq!(workflow, "deploy.yml");
                assert_eq!(git_ref.as_deref(), Some("$branch-ref"));
                assert_eq!(inputs.get("environment").map(String::as_str), Some("$1"));
            }
            other => panic!("expected WorkflowDispatch, got {other:?}"),
        }
    }

    #[test]
    fn parses_multiple_commands_and_defaults() {
        let src = r#"
            (defcommentcommand "test" :trigger "/test" :target (label "ci/run-tests"))
            ; a comment between forms
            (defcommentcommand "retest" :trigger "/retest" :target (label "ci/run-tests"))
        "#;
        let catalog = parse(src).expect("parses");
        assert_eq!(catalog.commands.len(), 2);
        assert_eq!(catalog.commands[0].min_permission, Permission::Write);
        assert!(catalog.commands[0].trust_pr_author);
    }

    #[test]
    fn missing_target_is_a_named_semantic_error() {
        let src = r#"(defcommentcommand "test" :trigger "/test")"#;
        let err = parse(src).unwrap_err();
        assert!(err.to_string().contains(":target"));
    }

    #[test]
    fn unterminated_paren_is_a_named_syntax_error() {
        let src = r#"(defcommentcommand "test" :trigger "/test""#;
        let err = parse(src).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("missing `)`") || err.to_string().to_lowercase().contains("end of input"));
    }

    #[test]
    fn unknown_target_kind_is_named() {
        let src = r#"(defcommentcommand "x" :trigger "/x" :target (bogus-kind "y"))"#;
        let err = parse(src).unwrap_err();
        assert!(err.to_string().contains("unknown :target kind"));
    }
}
