//! Runs one reviewer: `claude -p` with a JSON schema, the prompt on standard
//! input, and reads its answer and its token counts.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

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
    /// For a group that lists the sites it judges: its summary and sites, as JSON.
    pub sites: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The reviewer's JSON answer as printed, or why there was none.
    pub output: String,
    pub usage: Usage,
    /// The answer, or why there is none.
    pub answer: Result<Answer, String>,
}

/// The tokens one `claude -p` process used and what it cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// All input tokens: uncached, read from the cache and written to it.
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cached: i64,
    pub tokens_written: i64,
    /// `total_cost_usd` in millionths of a US dollar: what the process cost;
    /// 0 when it printed no cost, as a process that failed does.
    pub cost_microdollars: i64,
}

/// What `claude -p` printed: the tokens it used and its answer in the schema.
#[derive(Clone, Debug, PartialEq)]
pub struct Invoked {
    pub usage: Usage,
    pub body: Result<Value, String>,
}

/// Runs <program> as `claude` is run for a review: `-p` with <schema>,
/// <prompt> on standard input. With a <checkout> the reviewer runs in it with
/// read-only tools (Read, Grep, Glob); without one it has no tools. It never
/// loads the person's CLAUDE.md files: everything it is told is in the prompt.
pub fn invoke(
    program: &Path,
    model: &str,
    schema: &Value,
    system: &str,
    prompt_text: &str,
    checkout: Option<&Path>,
) -> Invoked {
    let schema = schema.to_string();
    let mut command = Command::new(program);
    command
        .args(["-p", "--output-format", "json", "--json-schema", &schema])
        .args(["--model", model])
        .args(["--no-session-persistence", "--disable-slash-commands"])
        .args(["--strict-mcp-config", "--system-prompt", system])
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match checkout {
        Some(dir) => {
            command
                .args(["--tools", "Read,Grep,Glob"])
                .args(["--allowedTools", "Read", "Grep", "Glob"])
                .current_dir(dir);
        }
        None => {
            command.args(["--tools", ""]);
        }
    }
    let failed = |why: String| Invoked {
        usage: Usage::default(),
        body: Err(why),
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
    envelope(&String::from_utf8_lossy(&output.stdout))
}

/// Reads the JSON `claude -p --output-format json` prints.
pub fn envelope(text: &str) -> Invoked {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Invoked {
            usage: Usage::default(),
            body: Err("the reviewer printed no JSON".to_string()),
        };
    };
    let count = |key: &str| value["usage"][key].as_i64().unwrap_or(0);
    let tokens_in = count("input_tokens")
        + count("cache_creation_input_tokens")
        + count("cache_read_input_tokens");
    let tokens_out = count("output_tokens");
    let body = body_of(&value);
    Invoked {
        usage: Usage {
            tokens_in,
            tokens_out,
            tokens_cached: count("cache_read_input_tokens"),
            tokens_written: count("cache_creation_input_tokens"),
            cost_microdollars: (value["total_cost_usd"].as_f64().unwrap_or(0.0) * 1_000_000.0)
                .round() as i64,
        },
        body,
    }
}

fn body_of(value: &Value) -> Result<Value, String> {
    if value["is_error"].as_bool().unwrap_or(false) {
        let said = value["result"].as_str().unwrap_or("no reason given");
        return Err(format!("the reviewer reported an error: {said}"));
    }
    match value.get("structured_output").filter(|v| v.is_object()) {
        Some(body) => Ok(body.clone()),
        None => value["result"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .ok_or_else(|| "the reviewer gave no answer in the schema".to_string()),
    }
}

/// Runs a group's reviewer and reads its answer: verdicts per rule, or, for a
/// group that lists sites, a summary and a verdict per site.
pub fn run(
    program: &Path,
    model: &str,
    names: &[String],
    enumerate: bool,
    parts: &prompt::Parts,
    checkout: Option<&Path>,
) -> Output {
    let invoked = invoke(
        program,
        model,
        &prompt::schema(),
        &parts.system,
        &parts.message,
        checkout,
    );
    output_of(invoked, names, enumerate)
}

fn output_of(invoked: Invoked, names: &[String], enumerate: bool) -> Output {
    let output = match &invoked.body {
        Ok(body) => body.to_string(),
        Err(why) => format!("no answer: {why}"),
    };
    Output {
        output,
        usage: invoked.usage,
        answer: invoked.body.and_then(|body| {
            if enumerate {
                sites_of(&body, names)
            } else {
                answer_of(&body, names)
            }
        }),
    }
}

/// Reads the answer of <text>, the JSON `claude -p` printed.
#[cfg(test)]
pub fn parse(text: &str, names: &[String]) -> Output {
    output_of(envelope(text), names, false)
}

fn answer_of(body: &Value, names: &[String]) -> Result<Answer, String> {
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
    Ok(Answer {
        verdicts,
        findings,
        sites: None,
    })
}

/// The answer of a group that lists the sites it judges: a summary, then
/// every site with its verdict. A finding site names its rule (a group of one
/// rule may leave it out). A rule is `finding` when any site is a finding for
/// it, else `none`.
fn sites_of(body: &Value, names: &[String]) -> Result<Answer, String> {
    let summary = body["summary"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or("the answer has no summary of the sites")?;
    let sites = body["sites"]
        .as_array()
        .ok_or("the answer has no sites list")?;
    let mut findings = Vec::new();
    for site in sites {
        let text = |key: &str| site[key].as_str().map(str::to_string);
        let file = text("file").ok_or("a site has no file")?;
        let code = text("code").ok_or("a site has no code")?;
        let line = site["line"].as_i64().ok_or("a site has no line")?;
        match site["verdict"].as_str() {
            Some("none") => {}
            Some("finding") => {
                let rule = match (text("rule"), names) {
                    (Some(rule), _) if names.contains(&rule) => rule,
                    (Some(rule), _) => {
                        return Err(format!(
                            "a finding site names {rule}, which is not in the group"
                        ));
                    }
                    (None, [only]) => only.clone(),
                    (None, _) => return Err("a finding site names no rule".to_string()),
                };
                findings.push(Raw {
                    rule,
                    clause: text("clause").ok_or("a finding site has no clause")?,
                    file,
                    line,
                    code,
                    problem: text("problem").ok_or("a finding site has no problem")?,
                });
            }
            _ => return Err("a site's verdict is not finding or none".to_string()),
        }
    }
    let verdicts = names
        .iter()
        .map(|name| {
            let verdict = if findings.iter().any(|f| &f.rule == name) {
                "finding"
            } else {
                "none"
            };
            (name.clone(), verdict.to_string())
        })
        .collect();
    Ok(Answer {
        verdicts,
        findings,
        sites: Some(json!({"summary": summary, "sites": sites}).to_string()),
    })
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
