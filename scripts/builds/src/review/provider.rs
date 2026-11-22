//! Selects the reviewer executable and model and connects the provider's CLI
//! protocol to the common schemas and checks.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::fail::{Context, Result};
use crate::review::call::{Invoked, Usage, envelope};

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

pub fn default_program() -> PathBuf {
    let program = std::env::var_os("BUILDS_REVIEWER").unwrap_or_else(|| {
        if std::env::var_os("CLAUDE_CODE_SESSION_ID").is_some() {
            "claude".into()
        } else if std::env::var_os("CODEX_SESSION_ID").is_some()
            || std::env::var_os("CODEX_THREAD_ID").is_some()
        {
            "codex".into()
        } else {
            "claude".into()
        }
    });
    PathBuf::from(program)
}

pub const TIERS: [&str; 3] = ["high", "medium", "fast"];

struct Model {
    name: String,
    effort: &'static str,
}

impl Model {
    fn of(provider: Provider, selection: &str) -> Self {
        let (name, effort) = match (provider, selection) {
            (Provider::Claude, "high") => ("opus", "high"),
            (Provider::Claude, "medium") => ("sonnet", "medium"),
            (Provider::Claude, "fast") => ("haiku", "low"),
            (Provider::Codex, "high") => ("gpt-6.2-astra", "high"),
            (Provider::Codex, "medium") => ("gpt-6.2-sol", "medium"),
            (Provider::Codex, "fast") => ("gpt-6.2-sol", "low"),
            (_, name) => (name, "high"),
        };
        Self {
            name: name.to_string(),
            effort,
        }
    }
}

pub(super) fn model_name(program: &Path, selection: &str) -> String {
    Model::of(Provider::of(program), selection).name
}

pub fn invoke(
    program: &Path,
    selection: &str,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Invoked {
    let provider = Provider::of(program);
    let model = Model::of(provider, selection);
    let called = match provider {
        Provider::Claude => claude(program, &model, schema, system, message, checkout),
        Provider::Codex => codex(program, &model, schema, system, message, checkout),
    };
    match called {
        Ok(invoked) => invoked,
        Err(error) => Invoked::failed(model.name, error.text()),
    }
}

fn run(mut command: Command, message: &str) -> Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| format!("running {program}"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    std::thread::scope(|scope| -> Result<Output> {
        let writer = scope.spawn(move || stdin.write_all(message.as_bytes()));
        let output = child
            .wait_with_output()
            .context(|| format!("waiting for {program}"))?;
        let sent = writer
            .join()
            .expect("the prompt writer only writes to a pipe");
        if output.status.success() {
            sent.context(|| format!("writing the prompt to {program}"))?;
        }
        Ok(output)
    })
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
    model: &Model,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Result<Invoked> {
    let schema = schema.to_string();
    let mut command = Command::new(program);
    command
        .args(["-p", "--output-format", "json", "--json-schema", &schema])
        .args(["--model", &model.name])
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
    let output = run(command, message)?;
    Ok(if output.status.success() {
        envelope(&String::from_utf8_lossy(&output.stdout), &model.name)
    } else {
        Invoked::failed(model.name.clone(), failed_output(program, &output))
    })
}

/// Holds the schema only while Codex reads it.
struct SchemaDirectory(PathBuf);

impl Drop for SchemaDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            crate::text::note(format!("could not remove {}: {error}", self.0.display()));
        }
    }
}

fn codex(
    program: &Path,
    model: &Model,
    schema: &Value,
    system: &str,
    message: &str,
    checkout: Option<&Path>,
) -> Result<Invoked> {
    let temporary = SchemaDirectory(crate::evict::make_temp_dir(
        &std::env::temp_dir(),
        "builds-review.",
    )?);
    let path = temporary.0.join("schema.json");
    let mut schema = schema.clone();
    require_properties(&mut schema);
    std::fs::write(&path, schema.to_string()).context(|| format!("writing {}", path.display()))?;
    let mut command = Command::new(program);
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
            "--disable",
            "multi_agent",
            "--disable",
            "image_generation",
        ])
        .args(["--disable", "goals", "--disable", "view_image"])
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
                Value::String(system.to_string())
            ),
        ])
        .args([
            "--model",
            &model.name,
            "-c",
            &format!("model_reasoning_effort=\"{}\"", model.effort),
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
                .current_dir(&temporary.0);
        }
    }
    let output = run(command, message)?;
    let mut invoked = codex_events(
        &String::from_utf8_lossy(&output.stdout),
        &model.name,
        checkout.is_some(),
    );
    if !output.status.success() {
        let why = match invoked.body {
            Err(why) => format!("{} exited with {}: {why}", program.display(), output.status),
            Ok(_) => failed_output(program, &output),
        };
        invoked.body = Err(why);
    }
    Ok(invoked)
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

fn codex_events(text: &str, model: &str, read_tools: bool) -> Invoked {
    let mut completed = None;
    let mut answer = None;
    let mut failure = None;
    for line in text.lines() {
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(error) => {
                return Invoked::failed(
                    model.to_string(),
                    format!("Codex printed an invalid event: {error}"),
                );
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
        cost_microdollars: 0,
    })
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
