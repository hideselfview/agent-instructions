use super::*;

fn names() -> Vec<String> {
    vec!["a".to_string(), "b".to_string()]
}

fn wrap(body: &str) -> String {
    format!(
        r#"{{"is_error":false,"usage":{{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":5,"output_tokens":7}},"structured_output":{body}}}"#
    )
}

#[test]
fn an_answer_gives_verdicts_findings_and_tokens() {
    let body = r#"{"rules":[{"rule":"a","verdict":"none","findings":[]},{"rule":"b","verdict":"finding","findings":[{"clause":"c","file":"f.rs","line":3,"code":"x","problem":"p"}]}]}"#;
    let out = parse(&wrap(body), &names());
    assert_eq!((out.tokens_in, out.tokens_out), (17, 7));
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
    let text = format!(r#"{{"is_error":false,"usage":{{}},"result":"{body}"}}"#);
    assert!(parse(&text, &names()).answer.is_ok());
    let error = r#"{"is_error":true,"result":"rate limited","usage":{}}"#;
    assert!(
        parse(error, &names())
            .answer
            .unwrap_err()
            .contains("rate limited")
    );
    assert!(parse("not json", &names()).answer.is_err());
}
