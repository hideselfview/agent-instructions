use super::*;

fn names() -> Vec<String> {
    vec!["a".to_string(), "b".to_string()]
}

fn wrap(body: &str) -> String {
    format!(
        r#"{{"is_error":false,"total_cost_usd":0.25,"usage":{{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"output_tokens":7}},"structured_output":{body}}}"#
    )
}

#[test]
fn an_answer_gives_verdicts_findings_and_tokens() {
    let body = r#"{"rules":[{"rule":"a","verdict":"none","findings":[]},{"rule":"b","verdict":"finding","findings":[{"clause":"c","file":"f.rs","line":3,"code":"x","problem":"p"}]}]}"#;
    let out = parse(&wrap(body), &names());
    assert_eq!(
        (
            out.usage.tokens.unwrap().input,
            out.usage.tokens.unwrap().output
        ),
        (17, 7)
    );
    assert_eq!(out.usage.cost_microdollars, Some(250_000));
    let answer = out.answer.unwrap();
    assert_eq!(
        answer.verdicts,
        [
            ("a".to_string(), "none".to_string()),
            ("b".to_string(), "finding".to_string())
        ]
    );
    assert_eq!(answer.findings.len(), 1);
    assert_eq!(answer.findings[0].rule, "b");
    assert_eq!(answer.findings[0].line, 3);
}

#[test]
fn an_answer_is_refused_when_it_skips_a_rule_or_contradicts_itself() {
    let skipped = r#"{"rules":[{"rule":"a","verdict":"none","findings":[]}]}"#;
    let why = parse(&wrap(skipped), &names()).answer.unwrap_err();
    assert!(why.contains("no verdict for b"), "{why}");
    let empty = r#"{"rules":[{"rule":"a","verdict":"none","findings":[]},{"rule":"b","verdict":"finding","findings":[]}]}"#;
    let why = parse(&wrap(empty), &names()).answer.unwrap_err();
    assert!(why.contains("finding with 0 findings"), "{why}");
    let stranger = r#"{"rules":[{"rule":"z","verdict":"none","findings":[]}]}"#;
    assert!(
        parse(&wrap(stranger), &names())
            .answer
            .unwrap_err()
            .contains("not in the group")
    );
}

#[test]
fn a_text_result_is_read_when_there_is_no_structured_output_and_an_error_is_named() {
    let body = r#"{\"rules\":[{\"rule\":\"a\",\"verdict\":\"none\",\"findings\":[]},{\"rule\":\"b\",\"verdict\":\"none\",\"findings\":[]}]}"#;
    let text = format!(
        r#"{{"is_error":false,"usage":{{"input_tokens":3,"output_tokens":1}},"result":"{body}"}}"#
    );
    let read = parse(&text, &names());
    assert!(read.answer.is_ok());
    assert_eq!(
        read.usage.cost_microdollars, None,
        "an envelope that names no cost adds none"
    );
    let error = r#"{"is_error":true,"result":"rate limited","usage":{}}"#;
    assert!(
        parse(error, &names())
            .answer
            .unwrap_err()
            .contains("rate limited")
    );
    assert!(parse("not json", &names()).answer.is_err());
}

#[test]
fn the_cache_read_and_write_tokens_are_kept_apart_from_the_rest_of_the_input() {
    let out = parse(&wrap(r#"{"rules":[]}"#), &[]);
    assert_eq!(
        (
            out.usage.tokens.unwrap().input,
            out.usage.tokens.unwrap().cached,
            out.usage.tokens.unwrap().written
        ),
        (17, 5, 10)
    );
}

#[test]
fn a_group_that_lists_sites_answers_with_a_summary_and_a_verdict_for_each_site() {
    let names = vec!["r".to_string()];
    let body = |sites: &str| format!(r#"{{"summary":"it filters a read's rows","sites":{sites}}}"#);
    let one = r#"[{"file":"a.rs","line":3,"kind":"filter","code":"x","verdict":"finding","clause":"Never.","problem":"p"},{"file":"b.rs","line":4,"kind":"count","code":"y","verdict":"none"}]"#;
    let answer = sites_of(&serde_json::from_str(&body(one)).unwrap(), &names).unwrap();
    assert_eq!(answer.verdicts, [("r".to_string(), "finding".to_string())]);
    assert_eq!(answer.findings.len(), 1);
    assert_eq!(answer.findings[0].file, "a.rs");
    assert!(answer.sites.unwrap().contains("it filters a read's rows"));
    let none = sites_of(&serde_json::from_str(&body("[]")).unwrap(), &names).unwrap();
    assert_eq!(none.verdicts, [("r".to_string(), "none".to_string())]);
    let no_summary = serde_json::json!({"sites": []});
    assert!(
        sites_of(&no_summary, &names)
            .unwrap_err()
            .contains("no summary")
    );
    let no_clause = r#"[{"file":"a.rs","line":3,"kind":"filter","code":"x","verdict":"finding","problem":"p"}]"#;
    assert!(
        sites_of(&serde_json::from_str(&body(no_clause)).unwrap(), &names)
            .unwrap_err()
            .contains("no clause")
    );
    // With two rules, a finding must name its rule, and one of them.
    let two = ["a".to_string(), "b".to_string()];
    let unnamed = r#"[{"file":"a.rs","line":3,"kind":"k","code":"x","verdict":"finding","clause":"c","problem":"p"}]"#;
    assert!(
        sites_of(&serde_json::from_str(&body(unnamed)).unwrap(), &two)
            .unwrap_err()
            .contains("names no rule")
    );
    let named = r#"[{"file":"a.rs","line":3,"kind":"k","code":"x","verdict":"finding","rule":"b","clause":"c","problem":"p"}]"#;
    let answer = sites_of(&serde_json::from_str(&body(named)).unwrap(), &two).unwrap();
    assert_eq!(
        answer.verdicts,
        vec![
            ("a".to_string(), "none".to_string()),
            ("b".to_string(), "finding".to_string())
        ]
    );
    assert_eq!(answer.findings[0].rule, "b");
}

#[test]
fn merging_usage_reports_overflow_and_keeps_other_recorded_counters() {
    let mut previous = Usage {
        cost_microdollars: Some(i64::MAX),
        tokens: Some(Tokens {
            input: i64::MAX,
            ..Tokens::ZERO
        }),
    };
    let new = Usage {
        cost_microdollars: Some(1),
        tokens: Some(Tokens {
            input: 1,
            ..Tokens::ZERO
        }),
    };
    let diagnostic = previous.merge(new).unwrap_err();
    assert!(diagnostic.contains("token total") && diagnostic.contains("review cost"));
    assert!(previous.tokens.is_none() && previous.cost_microdollars.is_none());
    assert_eq!(sum_tokens([Some(Tokens::ZERO), None]).unwrap(), None);
    assert_eq!(
        sum_tokens([Some(Tokens::ZERO)]).unwrap(),
        Some(Tokens::ZERO)
    );
}
