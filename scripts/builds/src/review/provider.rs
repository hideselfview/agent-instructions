//! Selects the reviewer executable and model and connects the provider's CLI
//! protocol to the common schemas and checks.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use crate::fail::{Context, Result};
use crate::review::call::{Invoked, Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn of(program: &Path) -> Self {
        match program.file_stem().and_then(|stem| stem.to_str()) {
            Some("codex") => Self::Codex,
            _ => Self::Claude,
        }
    }
}

/// A reviewer executable and the directory in which builds stages its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    executable: PathBuf,
    temporary: PathBuf,
    purpose: Purpose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    Check,
    CodeReview,
}

impl Program {
    pub fn new(executable: PathBuf, temporary: PathBuf) -> Self {
        Self {
            executable,
            temporary,
            purpose: Purpose::Check,
        }
    }

    pub fn for_code_review(&self) -> Self {
        Self {
            purpose: Purpose::CodeReview,
            ..self.clone()
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn with_executable(&self, executable: PathBuf) -> Self {
        Self {
            executable,
            ..self.clone()
        }
    }

    pub fn model_name(&self, selection: &Selection) -> String {
        self.model(selection).name().to_string()
    }

    pub fn setup(&self, selection: &Selection, prompt: String) -> String {
        match self.model(selection) {
            Model::Claude(_) => prompt,
            Model::Codex { effort, .. } => {
                format!("{prompt}\n# Codex\n\nreasoning effort: {effort}\n")
            }
        }
    }

    fn model(&self, selection: &Selection) -> Model {
        match Provider::of(&self.executable) {
            Provider::Claude => {
                let name = match selection {
                    Selection::Tier(Tier::High) => "opus",
                    Selection::Tier(Tier::Medium) => "sonnet",
                    Selection::Tier(Tier::Fast) => "haiku",
                    Selection::Named(name) => name,
                };
                Model::Claude(name.to_string())
            }
            Provider::Codex => {
                let (name, effort) = match (self.purpose, selection) {
                    (_, Selection::Named(name)) => (name.as_str(), "high"),
                    (Purpose::CodeReview, Selection::Tier(_)) => ("gpt-6.2-astra", "high"),
                    (Purpose::Check, Selection::Tier(Tier::High | Tier::Medium)) => {
                        ("gpt-6.2-sol", "medium")
                    }
                    (Purpose::Check, Selection::Tier(Tier::Fast)) => ("gpt-6.2-sol", "low"),
                };
                Model::Codex {
                    name: name.to_string(),
                    effort,
                }
            }
        }
    }
}

pub(super) fn default_program(
    explicit: Option<std::ffi::OsString>,
    claude_session: Option<&str>,
    codex_session: bool,
) -> PathBuf {
    if let Some(executable) = explicit {
        return PathBuf::from(executable);
    }
    if crate::ack::agent::claude_session(claude_session).is_none() && codex_session {
        PathBuf::from("codex")
    } else {
        PathBuf::from("claude")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    High,
    Medium,
    Fast,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Tier(Tier),
    Named(String),
}

impl std::fmt::Display for Selection {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Tier(Tier::High) => "high",
            Self::Tier(Tier::Medium) => "medium",
            Self::Tier(Tier::Fast) => "fast",
            Self::Named(name) => name,
        };
        out.write_str(name)
    }
}

enum Model {
    Claude(String),
    Codex { name: String, effort: &'static str },
}

impl Model {
    fn name(&self) -> &str {
        match self {
            Self::Claude(name) | Self::Codex { name, .. } => name,
        }
    }
}

pub fn invoke(
    program: &Program,
    selection: &Selection,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Invoked {
    let model = program.model(selection);
    let called = match &model {
        Model::Claude(name) => claude(
            program.executable(),
            name,
            schema,
            system,
            message,
            checkout,
        ),
        Model::Codex { name, effort } => {
            codex(program, name, effort, schema, system, message, checkout)
        }
    };
    match called {
        Ok(invoked) => invoked,
        Err(error) => Invoked::failed(
            format!("unknown (requested {})", model.name()),
            error.text(),
        ),
    }
}

fn run(command: &mut Command, message: &str) -> Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    let (output, written) = crate::child_input::run(command, message.as_bytes())
        .context(|| format!("running {program}"))?;
    if output.status.success() {
        written.context(|| format!("writing the prompt to {program}"))?;
    }
    Ok(output)
}

fn failed_output(program: &Path, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let said = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    format!(
        "{} exited with {}: {said}",
        program.display(),
        output.status
    )
}

fn claude(
    program: &Path,
    model: &str,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Result<Invoked> {
    let schema = schema.to_string();
    let mut command = Command::new(program);
    command
        .args(["-p", "--output-format", "json", "--json-schema", &schema])
        .args(["--model", model])
        .args(["--no-session-persistence", "--disable-slash-commands"])
        .args(["--strict-mcp-config", "--system-prompt", system])
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");
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
    let output = run(&mut command, message)?;
    Ok(if output.status.success() {
        claude_envelope(&output.stdout, model)
    } else {
        Invoked::failed(
            format!("unknown (requested {model})"),
            failed_output(program, &output),
        )
    })
}

/// Reads the JSON `claude -p --output-format json` prints.
pub(super) fn claude_envelope(text: &[u8], requested: &str) -> Invoked {
    let Ok(value) = serde_json::from_slice::<Value>(text) else {
        return Invoked::failed(
            format!("unknown (requested {requested})"),
            "the reviewer printed no JSON".to_string(),
        );
    };
    let count = |key: &str| value["usage"][key].as_i64().unwrap_or(0);
    let tokens_in = count("input_tokens")
        + count("cache_creation_input_tokens")
        + count("cache_read_input_tokens");
    let tokens_out = count("output_tokens");
    let body = body_of(&value);
    let model = value["modelUsage"]
        .as_object()
        .and_then(|models| {
            if models.len() == 1 {
                models.keys().next()
            } else {
                models.keys().find(|name| {
                    name.as_str() == requested || name.starts_with(&format!("claude-{requested}-"))
                })
            }
        })
        .map_or_else(|| format!("unknown (requested {requested})"), Clone::clone);
    Invoked {
        model,
        usage: Usage {
            tokens_in,
            tokens_out,
            tokens_cached: count("cache_read_input_tokens"),
            tokens_written: count("cache_creation_input_tokens"),
            cost_microdollars: value["total_cost_usd"]
                .as_f64()
                .map(|cost| (cost * 1_000_000.0).round() as i64),
        },
        body,
    }
}

fn body_of(value: &Value) -> std::result::Result<Value, String> {
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

fn codex(
    program: &Program,
    model: &str,
    effort: &str,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Result<Invoked> {
    let mut usage = Usage::default();
    let body = crate::evict::in_temp_dir(&program.temporary, "builds-review.", |temporary| {
        let path = temporary.join("schema.json");
        let mut schema = schema.clone();
        require_properties(&mut schema);
        std::fs::write(&path, schema.to_string())
            .context(|| format!("writing {}", path.display()))?;
        let instructions = format!(
            "{system}\n\nReturn null for optional fields these instructions say to omit. Null represents an omitted field in the output schema."
        );
        let mut command = Command::new(program.executable());
        command
            .args([
                "-a",
                "never",
                "exec",
                "--json",
                "--ephemeral",
                "--ignore-user-config",
            ])
            .args(["--sandbox", "read-only", "--skip-git-repo-check"])
            .args([
                "--disable",
                "plugins",
                "--disable",
                "hooks",
                "--disable",
                "apps",
            ])
            .args([
                "--disable",
                "multi_agent",
                "--disable",
                "image_generation",
                "--disable",
                "goals",
                "--disable",
                "view_image",
            ])
            .args([
                "-c",
                "project_doc_max_bytes=0",
                "-c",
                "web_search=\"disabled\"",
            ])
            .args([
                "-c",
                &format!("developer_instructions={}", Value::String(instructions)),
            ])
            .args([
                "--model",
                model,
                "-c",
                &format!("model_reasoning_effort=\"{effort}\""),
            ])
            .arg("--output-schema")
            .arg(path);
        match checkout {
            Some(dir) => {
                command.current_dir(dir);
            }
            None => {
                command
                    .args(["--disable", "shell_tool", "--disable", "unified_exec"])
                    .current_dir(temporary);
            }
        }
        let output = run(&mut command, message)?;
        let invoked = codex_events(&output.stdout, model, checkout.is_some());
        usage = invoked.usage;
        if !output.status.success() {
            let reason = failed_output(program.executable(), &output);
            let detail = match invoked.body {
                Err(why) if !output.stderr.is_empty() => {
                    format!("{reason}. Codex also reported {why}")
                }
                Err(why) => format!(
                    "{} exited with {}: {why}",
                    program.executable().display(),
                    output.status
                ),
                Ok(_) => reason,
            };
            return Err(crate::fail::Fail::Message(detail));
        }
        invoked.body.map_err(crate::fail::Fail::Message)
    });
    Ok(Invoked {
        model: model.to_string(),
        usage,
        body: body.map_err(|error| error.text()),
    })
}

/// OpenAI structured output requires every object property to be listed in
/// required. A property Builds' schema permits the model to omit becomes
/// nullable for the transport, then disappears from the returned object.
fn require_properties(schema: &mut Value) {
    if let Some(items) = schema.get_mut("items") {
        require_properties(items);
    }
    let required = schema["required"].as_array().cloned();
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (name, property) in properties.iter_mut() {
        require_properties(property);
        if !required
            .as_ref()
            .is_some_and(|keys| keys.iter().any(|key| key == name.as_str()))
        {
            *property = serde_json::json!({"anyOf": [property.take(), {"type": "null"}]});
        }
    }
    schema["required"] = Value::Array(properties.keys().cloned().map(Value::String).collect());
}

fn remove_null_properties(value: &mut Value) {
    if let Some(properties) = value.as_object_mut() {
        properties.retain(|_, value| !value.is_null());
        for property in properties.values_mut() {
            remove_null_properties(property);
        }
    } else if let Some(items) = value.as_array_mut() {
        for item in items {
            remove_null_properties(item);
        }
    }
}

fn codex_events(text: &[u8], model: &str, read_tools: bool) -> Invoked {
    let mut completed = None;
    let mut answer = None;
    let mut failure = None;
    for line in text
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let event: Value = match serde_json::from_slice(line) {
            Ok(event) => event,
            Err(error) => {
                failure.get_or_insert_with(|| format!("Codex printed an invalid event: {error}"));
                continue;
            }
        };
        match event["type"].as_str() {
            Some("item.completed") => match event["item"]["type"].as_str() {
                Some("agent_message") => {
                    answer = event["item"]["text"].as_str().map(str::to_string);
                }
                Some("reasoning") => {}
                Some("command_execution") if read_tools => {}
                Some(kind) => {
                    failure = Some(format!(
                        "Codex used a tool outside its review role ({kind})"
                    ))
                }
                None => {
                    failure = Some("Codex finished an item without naming what it did".to_string())
                }
            },
            Some("turn.completed") => completed = Some(event["usage"].clone()),
            Some("turn.failed") => {
                failure = Some(
                    event["error"]["message"]
                        .as_str()
                        .map_or_else(|| event["error"].to_string(), str::to_string),
                );
            }
            _ => {}
        }
    }
    let usage = completed
        .as_ref()
        .map_or_else(|| Ok(Usage::default()), codex_usage);
    let body = match (failure, completed, answer) {
        (Some(why), _, _) => Err(why),
        (None, Some(_), Some(answer)) => serde_json::from_str(&answer)
            .map(|mut body| {
                remove_null_properties(&mut body);
                body
            })
            .map_err(|error| format!("Codex did not answer in the schema: {error}")),
        _ => Err("Codex did not finish with a structured answer".to_string()),
    };
    match usage {
        Ok(usage) => Invoked {
            model: model.to_string(),
            usage,
            body,
        },
        Err(why) => Invoked::failed(model.to_string(), why),
    }
}

fn codex_usage(usage: &Value) -> std::result::Result<Usage, String> {
    let count = |key| {
        usage[key]
            .as_i64()
            .ok_or_else(|| format!("Codex did not report {key}"))
    };
    Ok(Usage {
        tokens_in: count("input_tokens")?,
        tokens_out: count("output_tokens")?,
        tokens_cached: count("cached_input_tokens")?,
        tokens_written: count("cache_write_input_tokens")?,
        cost_microdollars: None,
    })
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
