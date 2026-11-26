//! The lines of `claude -p --output-format stream-json --verbose` that
//! builds reads: the `system/init` line that says what Claude Code started,
//! each `assistant` line's usage, which gives the context of the agent's turn,
//! and the last `result` line, which holds the agent's report.

use crate::fail;
use crate::fail::Result;

/// What Claude Code says it started, from the `system/init` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Init {
    pub model: String,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<serde_json::Value>,
    pub skills: Vec<String>,
}

/// The agent's report, from the `result` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub text: String,
    /// The context window of the agent's model, from `modelUsage`.
    pub window: Option<u64>,
}

/// A line of the stream, as far as builds reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Init(Init),
    /// The agent's own turn, with the tokens of its context: the input, the
    /// cache-creation and the cache-read tokens together.
    Turn {
        context: u64,
    },
    Result(Report),
    Other,
}

/// Reads one line of the stream. A line that is no JSON object, and a line of
/// a type builds does not read, is `Other`. Fails for a line of a type builds
/// reads that lacks a field it needs.
pub fn parse(line: &str) -> Result<Line> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Ok(Line::Other);
    };
    match (str_of(&value, "type"), str_of(&value, "subtype")) {
        (Some("system"), Some("init")) => init(&value).map(Line::Init),
        (Some("assistant"), _) => turn(&value),
        (Some("result"), _) => report(&value).map(Line::Result),
        _ => Ok(Line::Other),
    }
}

fn str_of<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(serde_json::Value::as_str)
}

fn strings(value: &serde_json::Value, key: &str) -> Result<Vec<String>> {
    let Some(items) = value.get(key).and_then(serde_json::Value::as_array) else {
        fail!("the init line of the agent's stream has no list {key:?}");
    };
    items
        .iter()
        .map(|item| match item.as_str() {
            Some(text) => Ok(text.to_string()),
            None => fail!("the init line's {key:?} holds {item}, not a name"),
        })
        .collect()
}

fn init(value: &serde_json::Value) -> Result<Init> {
    let Some(model) = str_of(value, "model") else {
        fail!("the init line of the agent's stream names no model");
    };
    let Some(mcp_servers) = value
        .get("mcp_servers")
        .and_then(serde_json::Value::as_array)
    else {
        fail!("the init line of the agent's stream has no list \"mcp_servers\"");
    };
    Ok(Init {
        model: model.to_string(),
        tools: strings(value, "tools")?,
        mcp_servers: mcp_servers.clone(),
        skills: strings(value, "skills")?,
    })
}

fn turn(value: &serde_json::Value) -> Result<Line> {
    // A subagent's turn has the tool call that started it as its parent, and
    // its context is not the agent's.
    if value
        .get("parent_tool_use_id")
        .is_some_and(|parent| !parent.is_null())
    {
        return Ok(Line::Other);
    }
    let Some(usage) = value
        .get("message")
        .and_then(|message| message.get("usage"))
    else {
        return Ok(Line::Other);
    };
    let tokens = |key: &str| usage.get(key).and_then(serde_json::Value::as_u64);
    let (Some(input), Some(created), Some(read)) = (
        tokens("input_tokens"),
        tokens("cache_creation_input_tokens"),
        tokens("cache_read_input_tokens"),
    ) else {
        fail!("an assistant line of the agent's stream has a usage without its three input counts");
    };
    Ok(Line::Turn {
        context: input + created + read,
    })
}

fn report(value: &serde_json::Value) -> Result<Report> {
    let Some(text) = str_of(value, "result") else {
        fail!("the result line of the agent's stream holds no \"result\" text");
    };
    let window = value
        .get("modelUsage")
        .and_then(serde_json::Value::as_object)
        .and_then(|models| {
            models
                .values()
                .filter_map(|usage| usage.get("contextWindow")?.as_u64())
                .max()
        });
    Ok(Report {
        text: text.to_string(),
        window,
    })
}

#[cfg(test)]
#[path = "agent_stream_tests.rs"]
mod tests;
