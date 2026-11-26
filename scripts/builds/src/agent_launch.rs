//! The `claude` process builds starts for an agent: its argument list, which
//! builds makes from the role and the tier and never from text a caller
//! passes, the environment it starts in, the check of what Claude Code says
//! it started, and the reading of its output.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::agent_stream::{self, Init, Line};
use crate::fail;
use crate::fail::{Context, Result};

/// The eight tools of a dispatched agent. Claude Code starts no Skill tool
/// under `--disable-slash-commands`, so the list names none.
pub const TOOLS: [&str; 8] = [
    "Bash",
    "Edit",
    "LSP",
    "Monitor",
    "Read",
    "TaskStop",
    "ToolSearch",
    "Write",
];

/// The argument list of `claude` for a new agent of session <session> that runs
/// <model>, named <name> in Claude Code's resume list. A planner and an implementing agent take the
/// same flags: no MCP server, no skill, the eight tools, no permission
/// prompts, and the stream-json output builds reads. The prompt goes on
/// standard input, so no `ps` line holds it.
pub fn arguments(model: &str, name: &str, session: &str) -> Vec<String> {
    let mut arguments: Vec<String> = [
        "-p",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--tools",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    arguments.push(TOOLS.join(","));
    arguments.extend(
        [
            "--dangerously-skip-permissions",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
        ]
        .into_iter()
        .map(str::to_string),
    );
    arguments.push(model.to_string());
    arguments.extend(["--name".to_string(), name.to_string()]);
    arguments.extend(["--session-id".to_string(), session.to_string()]);
    arguments
}

/// The command that runs <program> with <arguments> in <folder>, in an
/// environment with no caller's identity and no inbox: the agent is a new
/// session of its own.
pub fn command(program: &Path, arguments: &[String], folder: &Path) -> Command {
    let mut command = Command::new(program);
    command.args(arguments).current_dir(folder);
    for name in crate::ack::agent::IDENTITY
        .iter()
        .chain(&crate::ack::agent::CLAUDE_CODE)
    {
        command.env_remove(name);
    }
    command
}

/// Fails when what Claude Code started differs from what builds asked for:
/// another model, a tool the list lacks or adds, an MCP server or a skill. A
/// Claude Code update that changes what a flag does shows here.
pub fn check_init(model: &str, init: &Init) -> Result<()> {
    let mut faults = Vec::new();
    if init.model != model {
        faults.push(format!(
            "it runs {} and builds asked for {model}",
            init.model
        ));
    }
    let mut tools = init.tools.clone();
    tools.sort();
    if tools != TOOLS {
        faults.push(format!(
            "its tools are {} and builds asked for {}",
            init.tools.join(", "),
            TOOLS.join(", ")
        ));
    }
    if !init.mcp_servers.is_empty() {
        faults.push(format!("it started {} MCP servers", init.mcp_servers.len()));
    }
    if !init.skills.is_empty() {
        faults.push(format!("it loaded the skills {}", init.skills.join(", ")));
    }
    if faults.is_empty() {
        return Ok(());
    }
    fail!(
        "Claude Code started the agent differently from the flags builds passed: {}. builds stopped the agent",
        faults.join("; ")
    );
}

/// The log of an agent: each line of the stream, and each line of claude's
/// standard error as a JSON line of type `stderr`, so that every line is
/// JSON.
#[derive(Clone)]
pub struct Log(Arc<Mutex<std::fs::File>>);

impl Log {
    /// The log at <path>, which new lines are appended to.
    pub fn append(path: &Path) -> Result<Log> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context(|| format!("opening {}", path.display()))?;
        Ok(Log(Arc::new(Mutex::new(file))))
    }

    fn line(&self, line: &str) {
        let mut file = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // A log that cannot be written must not stop the agent.
        let _ = writeln!(file, "{line}");
    }
}

/// A `claude` process builds started, and the threads that feed and drain it.
pub struct Started {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
    threads: Vec<std::thread::JoinHandle<()>>,
    log: Log,
}

/// Starts <command> and gives it <prompt> on standard input.
pub fn start(mut command: Command, prompt: &str, log: Log) -> Result<Started> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context(|| format!("starting {}", command.get_program().to_string_lossy()))?;
    let mut threads = Vec::new();
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let prompt = prompt.to_string();
    threads.push(std::thread::spawn(move || {
        // claude reads standard input to its end, then answers; a write that
        // fails means it ended, which the wait reports.
        let _ = stdin.write_all(prompt.as_bytes());
    }));
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_log = log.clone();
    threads.push(std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            stderr_log.line(&serde_json::json!({ "type": "stderr", "text": line }).to_string());
        }
    }));
    let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    Ok(Started {
        child,
        stdout,
        threads,
        log,
    })
}

impl Started {
    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Kills the process and waits for it, for a start that went wrong after
    /// the process began.
    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Reads the stream to its end, writing each line to the log and giving
    /// each parsed line to <each>. When <each> fails or a line cannot be
    /// read, kills the process, waits for it and returns the failure.
    pub fn read(&mut self, mut each: impl FnMut(Line) -> Result<()>) -> Result<()> {
        let mut text = String::new();
        loop {
            text.clear();
            let read = self
                .stdout
                .read_line(&mut text)
                .context(|| "reading the agent's output".to_string());
            let step = read.and_then(|bytes| {
                if bytes == 0 {
                    return Ok(false);
                }
                let line = text.trim_end_matches('\n');
                self.log.line(line);
                each(agent_stream::parse(line)?)?;
                Ok(true)
            });
            match step {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(error);
                }
            }
        }
    }

    /// Waits for the process to end and for its threads, and returns its
    /// exit code: the signal's number added to 128 for a process a signal
    /// ended.
    pub fn finish(mut self) -> Result<i32> {
        let status = self
            .child
            .wait()
            .context(|| "waiting for the agent".to_string())?;
        for thread in self.threads {
            let _ = thread.join();
        }
        use std::os::unix::process::ExitStatusExt;
        Ok(status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(1))
    }
}

#[cfg(test)]
#[path = "agent_launch_tests.rs"]
mod tests;
