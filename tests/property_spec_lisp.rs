//! Property tests over `spec_lisp::parse` — the untrusted-input boundary.
//! A consuming repo's `.github/aldrava.lisp` is attacker-adjacent (anyone who
//! can open a PR can propose an edit to it), so the parser's one
//! non-negotiable property is: it never panics, on any input whatsoever.

use aldrava::spec::{DispatchTarget, Permission};
use aldrava::spec_lisp::parse;
use proptest::prelude::*;

fn safe_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,12}"
}

fn safe_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _/.:-]{0,20}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The parser's one load-bearing robustness property: arbitrary bytes
    /// never panic it, they only ever produce a typed `LintError`.
    #[test]
    fn parser_never_panics_on_arbitrary_input(s in ".{0,300}") {
        let _ = parse(&s);
    }

    /// A well-formed label-target command round-trips its name, trigger,
    /// and label through the parser byte-for-byte, with the documented
    /// defaults applied when the optional keywords are omitted.
    #[test]
    fn label_command_round_trips_name_trigger_and_label(
        name in safe_ident(),
        trigger in safe_string(),
        label in safe_string(),
    ) {
        let src = format!(
            "(defcommentcommand \"{name}\" :trigger \"{trigger}\" :target (label \"{label}\"))"
        );
        let catalog = parse(&src).expect("a well-formed catalog must parse");
        prop_assert_eq!(catalog.commands.len(), 1);
        let cmd = &catalog.commands[0];
        prop_assert_eq!(&cmd.name, &name);
        prop_assert_eq!(&cmd.trigger, &trigger);
        prop_assert_eq!(cmd.min_permission, Permission::Write);
        prop_assert!(cmd.trust_pr_author);
        prop_assert_eq!(&cmd.target, &DispatchTarget::Label { name: label });
    }

    /// An `:allowlist` of any length round-trips as an ordered list of
    /// logins, never deduplicated, never reordered.
    #[test]
    fn allowlist_of_arbitrary_length_round_trips(
        logins in proptest::collection::vec(safe_ident(), 0..8),
    ) {
        let allowlist_src = logins
            .iter()
            .map(|l| format!("\"{l}\""))
            .collect::<Vec<_>>()
            .join(" ");
        let src = format!(
            "(defcommentcommand \"x\" :trigger \"/x\" :allowlist ({allowlist_src}) :target (label \"y\"))"
        );
        let catalog = parse(&src).expect("a well-formed catalog must parse");
        prop_assert_eq!(&catalog.commands[0].allowlist, &logins);
    }

    /// Every declared permission keyword round-trips to its exact typed
    /// variant — no permission silently maps to another.
    #[test]
    fn min_permission_keyword_round_trips(idx in 0..6usize) {
        let keywords = ["none", "read", "triage", "write", "maintain", "admin"];
        let expected = [
            Permission::None, Permission::Read, Permission::Triage,
            Permission::Write, Permission::Maintain, Permission::Admin,
        ];
        let src = format!(
            "(defcommentcommand \"x\" :trigger \"/x\" :min-permission {} :target (label \"y\"))",
            keywords[idx]
        );
        let catalog = parse(&src).expect("a well-formed catalog must parse");
        prop_assert_eq!(catalog.commands[0].min_permission, expected[idx]);
    }

    /// A catalog of N well-formed commands parses to exactly N entries, in
    /// source order — no form is dropped, merged, or reordered.
    #[test]
    fn multiple_commands_parse_to_the_same_count_in_order(
        names in proptest::collection::vec(safe_ident(), 1..6),
    ) {
        let src = names
            .iter()
            .enumerate()
            .map(|(i, n)| format!(
                "(defcommentcommand \"{n}\" :trigger \"/t{i}\" :target (label \"l\"))"
            ))
            .collect::<Vec<_>>()
            .join("\n");
        let catalog = parse(&src).expect("a well-formed catalog must parse");
        let got: Vec<&str> = catalog.commands.iter().map(|c| c.name.as_str()).collect();
        let want: Vec<&str> = names.iter().map(String::as_str).collect();
        prop_assert_eq!(got, want);
    }
}
