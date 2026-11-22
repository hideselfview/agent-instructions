//! Selects the reviewer executable and model and connects the provider's CLI
//! protocol to the common schemas and checks.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use crate::fail::{Context, Result};
use crate::review::call::{Invoked, Tokens, Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
}

/// An executable and the protocol Builds uses to call it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Executable {
    Claude(PathBuf),
    Codex { path: PathBuf, purpose: Purpose },
}

impl Executable {
    fn of(path: PathBuf) -> Self {
        match path.file_stem().and_then(|stem| stem.to_str()) {
            Some("codex") => Self::Codex {
                path,
                purpose: Purpose::Check,
            },
            _ => Self::Claude(path),
        }
    }

    fn provider(&self) -> Provider {
        match self {
            Self::Claude(_) => Provider::Claude,
            Self::Codex { .. } => Provider::Codex,
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Claude(path) | Self::Codex { path, .. } => path,
        }
    }
}

/// A reviewer executable and the directory in which builds stages its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    executable: Executable,
    temporary: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    Check,
    CodeReview,
}

impl Program {
    pub fn new(executable: PathBuf, temporary: PathBuf) -> Self {
        Self {
            executable: Executable::of(executable),
            temporary,
        }
    }

    pub(super) fn for_branch_review(&self, reads_code: impl FnOnce() -> bool) -> Self {
        match &self.executable {
            Executable::Codex { path, .. } if reads_code() => Self {
                executable: Executable::Codex {
                    path: path.clone(),
                    purpose: Purpose::CodeReview,
                },
                temporary: self.temporary.clone(),
            },
            _ => self.clone(),
        }
    }

    pub fn executable(&self) -> &Path {
        self.executable.path()
    }

    pub fn with_executable(&self, executable: PathBuf) -> Self {
        Self {
            executable: Executable::of(executable),
            ..self.clone()
        }
    }

    pub(super) fn select(&self, selection: &Selection) -> Selected<'_> {
        Selected {
            program: self,
            model: self.model(selection),
        }
    }

    pub fn schema<'a>(&self, schema: &'a Value) -> Cow<'a, Value> {
        match self.executable.provider() {
            Provider::Claude => Cow::Borrowed(schema),
            Provider::Codex => {
                let mut schema = schema.clone();
                require_properties(&mut schema);
                Cow::Owned(schema)
            }
        }
    }

    pub fn instructions<'a>(&self, system: &'a str) -> Cow<'a, str> {
        match self.executable.provider() {
            Provider::Claude => Cow::Borrowed(system),
            Provider::Codex => Cow::Owned(format!(
                "{system}\n\nReturn null for optional fields these instructions say to omit. Null represents an omitted field in the output schema."
            )),
        }
    }

    fn model(&self, selection: &Selection) -> Model {
        match &self.executable {
            Executable::Claude(_) => {
                let name = match selection {
                    Selection::Tier(Tier::High) => "opus",
                    Selection::Tier(Tier::Medium) => "sonnet",
                    Selection::Tier(Tier::Fast) => "haiku",
                    Selection::Named(name) => name,
                };
                Model::Claude(name.to_string())
            }
            Executable::Codex { purpose, .. } => {
                if let Selection::Named(requested) = selection
                    && requested != CODEX_MODEL
                {
                    return Model::UnsupportedCodex(requested.clone());
                }
                let effort = match (purpose, selection) {
                    (_, Selection::Named(_)) | (Purpose::CodeReview, Selection::Tier(_)) => "high",
                    (Purpose::Check, Selection::Tier(Tier::High | Tier::Medium)) => "medium",
                    (Purpose::Check, Selection::Tier(Tier::Fast)) => "low",
                };
                Model::Codex { effort }
            }
        }
    }
}

pub(super) fn default_program(
    explicit: Option<std::ffi::OsString>,
    session: &crate::ack::agent::Session,
) -> PathBuf {
    if let Some(executable) = explicit {
        return PathBuf::from(executable);
    }
    if matches!(session, crate::ack::agent::Session::Codex) {
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

const CODEX_MODEL: &str = "gpt-6.2-sol";

enum Model {
    Claude(String),
    Codex { effort: &'static str },
    UnsupportedCodex(String),
}

impl Model {
    fn requested_name(&self) -> &str {
        match self {
            Self::Claude(name) | Self::UnsupportedCodex(name) => name,
            Self::Codex { .. } => CODEX_MODEL,
        }
    }

    fn unavailable(&self) -> String {
        unknown_model(self.requested_name())
    }
}

fn unknown_model(requested: &str) -> String {
    format!("unknown (requested {requested})")
}

pub fn invoke(
    program: &Program,
    selection: &Selection,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Invoked {
    program
        .select(selection)
        .invoke(schema, system, message, checkout)
}

/// The chosen provider and model for one review or check.
pub(super) struct Selected<'a> {
    program: &'a Program,
    model: Model,
}

impl Selected<'_> {
    pub(super) fn model_name(&self) -> String {
        match self.model {
            Model::UnsupportedCodex(_) => self.model.unavailable(),
            _ => self.model.requested_name().to_string(),
        }
    }

    pub(super) fn program(&self) -> &Program {
        self.program
    }

    pub(super) fn setup(&self, prompt: String) -> String {
        match self.model {
            Model::Claude(_) | Model::UnsupportedCodex(_) => prompt,
            Model::Codex { effort } => format!("{prompt}\n# Codex\n\nreasoning effort: {effort}\n"),
        }
    }

    pub(super) fn invoke(
        &self,
        schema: &Value,
        system: &str,
        message: &str,
        checkout: Option<&Path>,
    ) -> Invoked {
        let called = match &self.model {
            Model::Claude(name) => claude(
                self.program.executable(),
                name,
                schema,
                system,
                message,
                checkout,
            ),
            Model::Codex { effort } => {
                codex(self.program, effort, schema, system, message, checkout)
            }
            Model::UnsupportedCodex(requested) => {
                return Invoked::failed(
                    self.model.unavailable(),
                    format!(
                        "Codex uses {CODEX_MODEL}. Replace --model {requested} with --model {CODEX_MODEL} or omit --model."
                    ),
                );
            }
        };
        match called {
            Ok(invoked) => invoked,
            Err(error) => Invoked::failed(self.model.unavailable(), error.text()),
        }
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
    let mut invoked = claude_envelope(&output.stdout, model);
    if !output.status.success() {
        attach_exit(program, &output, &mut invoked.body);
    }
    Ok(invoked)
}

/// Reads the JSON `claude -p --output-format json` prints.
pub(super) fn claude_envelope(text: &[u8], requested: &str) -> Invoked {
    let Ok(value) = serde_json::from_slice::<Value>(text) else {
        return Invoked::failed(
            unknown_model(requested),
            "the reviewer printed no JSON".to_string(),
        );
    };
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
        .map_or_else(|| unknown_model(requested), Clone::clone);
    let mut body = body_of(&value);
    let mut diagnostics = Vec::new();
    let tokens = reported_tokens(
        value.get("usage"),
        Provider::Claude,
        &mut body,
        &mut diagnostics,
    );
    let cost_microdollars = match claude_cost(&value) {
        Ok(cost) => cost,
        Err(why) => {
            add_diagnostic(&mut body, why);
            None
        }
    };
    Invoked {
        model,
        usage: Usage {
            tokens,
            cost_microdollars,
        },
        body,
        diagnostics,
    }
}

fn add_diagnostic(body: &mut std::result::Result<Value, String>, diagnostic: String) {
    *body = Err(match body {
        Ok(_) => diagnostic,
        Err(why) => format!("{why}. Also, {diagnostic}"),
    });
}

fn attach_exit(program: &Path, output: &Output, body: &mut std::result::Result<Value, String>) {
    let exit = failed_output(program, output);
    *body = Err(match body {
        Err(why) => format!("{exit}\nReviewer reported: {why}"),
        Ok(_) => exit,
    });
}

fn reported_count(usage: &Value, key: &str) -> std::result::Result<Option<i64>, String> {
    usage
        .get(key)
        .map(|value| {
            value
                .as_i64()
                .filter(|count| *count >= 0)
                .ok_or_else(|| format!("the reviewer reported an invalid {key}: {value}"))
        })
        .transpose()
}

fn reported_tokens(
    usage: Option<&Value>,
    protocol: Provider,
    body: &mut std::result::Result<Value, String>,
    diagnostics: &mut Vec<String>,
) -> Tokens {
    let Some(usage) = usage else {
        let provider = match protocol {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
        };
        diagnostics.push(format!("{provider} omitted its usage report"));
        return Tokens::default();
    };
    if !usage.is_object() {
        add_diagnostic(body, "the reviewer reported invalid usage".to_string());
        return Tokens::default();
    }
    let mut count = |key| match reported_count(usage, key) {
        Ok(None) => {
            diagnostics.push(format!("the reviewer omitted {key} from its usage report"));
            None
        }
        Ok(Some(value)) => Some(value),
        Err(why) => {
            add_diagnostic(body, why);
            None
        }
    };
    let (cache_read, cache_write) = match protocol {
        Provider::Claude => ("cache_read_input_tokens", "cache_creation_input_tokens"),
        Provider::Codex => ("cached_input_tokens", "cache_write_input_tokens"),
    };
    let reported_input = count("input_tokens");
    let output = count("output_tokens");
    let cached = count(cache_read);
    let written = count(cache_write);
    let reasoning = match protocol {
        Provider::Codex => reasoning_count(usage, output, diagnostics),
        Provider::Claude => None,
    };
    let computed = match (protocol, reported_input) {
        (Provider::Claude, Some(input)) => match (cached, written) {
            (Some(cached), Some(written)) => {
                let total = input
                    .checked_add(cached)
                    .and_then(|sum| sum.checked_add(written));
                if total.is_none() {
                    add_diagnostic(
                        body,
                        "Claude's total input tokens exceed the supported range".to_string(),
                    );
                }
                total
            }
            _ => None,
        },
        (Provider::Codex, Some(input)) => {
            let known_cache: i128 = [cached, written]
                .into_iter()
                .flatten()
                .map(i128::from)
                .sum();
            if known_cache > i128::from(input) {
                add_diagnostic(
                    body,
                    "Codex reported more input tokens in the cache than total input tokens"
                        .to_string(),
                );
                None
            } else {
                cached
                    .zip(written)
                    .map(|(cached, written)| input - cached - written)
            }
        }
        (_, None) => None,
    };
    let (input, uncached) = match protocol {
        Provider::Claude => (computed, reported_input),
        Provider::Codex => (reported_input, computed),
    };
    Tokens {
        input,
        output,
        cached,
        written,
        uncached,
        reasoning,
    }
}

fn reasoning_count(
    usage: &Value,
    output: Option<i64>,
    diagnostics: &mut Vec<String>,
) -> Option<i64> {
    match reported_count(usage, "reasoning_output_tokens") {
        Ok(Some(count)) if output.is_some_and(|output| count > output) => {
            diagnostics.push(format!(
                "Codex reported {count} reasoning tokens, more than its total output count"
            ));
            None
        }
        Ok(count) => count,
        Err(why) => {
            diagnostics.push(why);
            None
        }
    }
}

fn claude_cost(envelope: &Value) -> std::result::Result<Option<i64>, String> {
    envelope
        .get("total_cost_usd")
        .map(|value| {
            let micros = value
                .as_f64()
                .filter(|cost| *cost >= 0.0)
                .map(|cost| (cost * 1_000_000.0).round())
                .filter(|cost| *cost < i64::MAX as f64)
                .ok_or_else(|| format!("Claude reported invalid cost: {value}"))?;
            Ok(micros as i64)
        })
        .transpose()
}

fn body_of(value: &Value) -> std::result::Result<Value, String> {
    match value.get("is_error") {
        Some(Value::Bool(true)) => {
            let said = value
                .get("result")
                .ok_or("the reviewer reported an error without a reason")?;
            return Err(match said.as_str() {
                Some(reason) => format!("the reviewer reported an error: {reason}"),
                None => format!("the reviewer reported an error: {said}"),
            });
        }
        Some(Value::Bool(false)) => {}
        None => return Err("the reviewer omitted is_error".to_string()),
        Some(value) => return Err(format!("the reviewer reported invalid is_error: {value}")),
    }
    match value.get("structured_output") {
        Some(body) if body.is_object() => Ok(body.clone()),
        Some(body) => Err(format!(
            "the reviewer reported invalid structured_output: {body}"
        )),
        None => {
            let text = value["result"]
                .as_str()
                .ok_or("the reviewer gave no answer in the schema")?;
            serde_json::from_str(text)
                .map_err(|error| format!("the reviewer gave an invalid answer: {error}"))
        }
    }
}

fn codex(
    program: &Program,
    effort: &str,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Result<Invoked> {
    let mut invoked = None;
    let result = crate::evict::in_temp_dir(&program.temporary, "builds-review.", |temporary| {
        let path = temporary.join("schema.json");
        std::fs::write(&path, program.schema(schema).to_string())
            .context(|| format!("writing {}", path.display()))?;
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
                &format!(
                    "developer_instructions={}",
                    Value::String(program.instructions(system).into_owned())
                ),
            ])
            .args([
                "--model",
                CODEX_MODEL,
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
        let Decoded {
            completion,
            mut body,
            diagnostics,
        } = codex_events(&output.stdout, checkout.is_some());
        if !output.status.success() {
            attach_exit(program.executable(), &output, &mut body);
        }
        invoked = Some(Invoked {
            model: match completion {
                Completion::Completed(_) => CODEX_MODEL.to_string(),
                Completion::Incomplete => unknown_model(CODEX_MODEL),
            },
            usage: Usage {
                tokens: completion.tokens(),
                cost_microdollars: None,
            },
            body,
            diagnostics,
        });
        Ok(())
    });
    match (invoked, result) {
        (Some(response), Ok(())) => Ok(response),
        (Some(mut response), Err(error)) => {
            response.diagnostics.push(error.text());
            Ok(response)
        }
        (None, Err(error)) => Err(error),
        (None, Ok(())) => unreachable!("a completed Codex invocation supplies its result"),
    }
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

enum Completion {
    Incomplete,
    Completed(Tokens),
}

impl Completion {
    fn tokens(&self) -> Tokens {
        match self {
            Self::Incomplete => Tokens::default(),
            Self::Completed(tokens) => *tokens,
        }
    }
}

struct Decoded {
    completion: Completion,
    body: std::result::Result<Value, String>,
    diagnostics: Vec<String>,
}

fn codex_events(text: &[u8], read_tools: bool) -> Decoded {
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
                failure.get_or_insert_with(|| {
                    format!(
                        "Codex printed an invalid event ({error}): {}",
                        String::from_utf8_lossy(line)
                            .chars()
                            .take(240)
                            .collect::<String>()
                    )
                });
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
            Some("turn.completed") => completed = Some(event.get("usage").cloned()),
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
    let mut body = match (failure, completed.is_some(), answer) {
        (Some(why), _, _) => Err(why),
        (None, true, Some(answer)) => serde_json::from_str(&answer)
            .map(|mut body| {
                remove_null_properties(&mut body);
                body
            })
            .map_err(|error| format!("Codex did not answer in the schema: {error}")),
        _ => Err("Codex did not finish with a structured answer".to_string()),
    };
    let mut diagnostics = Vec::new();
    let completion = match completed {
        Some(usage) => Completion::Completed(reported_tokens(
            usage.as_ref(),
            Provider::Codex,
            &mut body,
            &mut diagnostics,
        )),
        None => Completion::Incomplete,
    };
    Decoded {
        completion,
        body,
        diagnostics,
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
