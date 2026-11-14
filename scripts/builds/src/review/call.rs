//! Runs one reviewer: `claude -p` with a JSON schema, the prompt on standard
//! input, and reads its answer and its token counts.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::review::prompt;

/// One finding as the reviewer wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raw {
    pub rule: String,
    pub clause: String,
    pub file: String,
    pub line: i64,
    pub code: String,
    pub problem: String,
}

/// What a reviewer answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// (rule, `finding` or `none`).
    pub verdicts: Vec<(String, String)>,
    pub findings: Vec<Raw>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// The answer, or why there is none.
    pub answer: Result<Answer, String>,
}

/// Runs <program> as `claude` is run for a review. The program gets the
/// flags below and <prompt> on standard input.
pub fn run(program: &Path, model: &str, names: &[String], prompt_text: &str) -> Output {
    let schema = prompt::schema(names).to_string();
    let mut command = Command::new(program);
    command
        .args(["-p", "--output-format", "json", "--json-schema", &schema])
        .args(["--model", model, "--tools", ""])
        .args(["--no-session-persistence", "--disable-slash-commands"])
        .args(["--strict-mcp-config", "--system-prompt", prompt::SYSTEM])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let failed = |why: String| Output {
        tokens_in: 0,
        tokens_out: 0,
        answer: Err(why),
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(format!("running {}: {error}", program.display())),
    };
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let text = prompt_text.to_string();
    // The prompt can be larger than the pipe, so it is written while the
    // answer is read.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(text.as_bytes());
    });
    let output = child.wait_with_output();
    let _ = writer.join();
    let output = match output {
        Ok(output) => output,
        Err(error) => return failed(format!("waiting for {}: {error}", program.display())),
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return failed(format!(
            "{} exited with {}: {}",
            program.display(),
            output.status,
            said.trim()
        ));
    }
    parse(&String::from_utf8_lossy(&output.stdout), names)
}

/// Reads the JSON `claude -p --output-format json` prints.
pub fn parse(text: &str, names: &[String]) -> Output {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Output {
            tokens_in: 0,
            tokens_out: 0,
            answer: Err("the reviewer printed no JSON".to_string()),
        };
    };
    let count = |key: &str| value["usage"][key].as_i64().unwrap_or(0);
    let tokens_in = count("input_tokens")
        + count("cache_creation_input_tokens")
        + count("cache_read_input_tokens");
    let tokens_out = count("output_tokens");
    let answer = answer_of(&value, names);
    Output {
        tokens_in,
        tokens_out,
        answer,
    }
}

fn answer_of(value: &Value, names: &[String]) -> Result<Answer, String> {
    if value["is_error"].as_bool().unwrap_or(false) {
        let said = value["result"].as_str().unwrap_or("no reason given");
        return Err(format!("the reviewer reported an error: {said}"));
    }
    let body: Value = match value.get("structured_output").filter(|v| v.is_object()) {
        Some(body) => body.clone(),
        None => value["result"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .ok_or("the reviewer gave no answer in the schema")?,
    };
    let rules = body["rules"]
        .as_array()
        .ok_or("the answer has no rules list")?;
    let mut verdicts: Vec<(String, String)> = Vec::new();
    let mut findings = Vec::new();
    for entry in rules {
        let rule = entry["rule"].as_str().ok_or("a verdict names no rule")?;
        let verdict = entry["verdict"].as_str().ok_or("a verdict says nothing")?;
        if !names.iter().any(|n| n == rule) {
            return Err(format!(
                "the answer has a verdict for {rule}, which is not in the group"
            ));
        }
        if verdict != "finding" && verdict != "none" {
            return Err(format!(
                "the verdict for {rule} is {verdict}, not finding or none"
            ));
        }
        if verdicts.iter().any(|(r, _)| r == rule) {
            return Err(format!("the answer has two verdicts for {rule}"));
        }
        let listed = entry["findings"].as_array().cloned().unwrap_or_default();
        if (verdict == "finding") == listed.is_empty() {
            return Err(format!(
                "the verdict for {rule} is {verdict} with {} findings",
                listed.len()
            ));
        }
        for f in listed {
            let text = |key: &str| f[key].as_str().map(str::to_string);
            findings.push(Raw {
                rule: rule.to_string(),
                clause: text("clause").ok_or("a finding has no clause")?,
                file: text("file").ok_or("a finding has no file")?,
                line: f["line"].as_i64().ok_or("a finding has no line")?,
                code: text("code").ok_or("a finding has no code")?,
                problem: text("problem").ok_or("a finding has no problem")?,
            });
        }
        verdicts.push((rule.to_string(), verdict.to_string()));
    }
    let missing: Vec<&str> = names
        .iter()
        .filter(|n| !verdicts.iter().any(|(r, _)| r == *n))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the answer has no verdict for {}",
            missing.join(", ")
        ));
    }
    Ok(Answer { verdicts, findings })
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
