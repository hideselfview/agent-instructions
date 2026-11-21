//! Whether the shells that agents' tools started are doing real work. Many
//! agents run commands at once, each in a shell its tool starts; some of those
//! are junk: a wait loop, a search over the whole disk, a copy of a command
//! already running in the same worktree, a command stuck on input or a lock.
//! `builds health` lists them, and `builds health --stop-junk` stops each one's
//! whole tree by PID.
//!
//! An agent shell looks, from outside, like Claude Code's: `/bin/zsh -c source
//! ~/.claude/shell-snapshots/snapshot-zsh-….sh … && eval '<command>' <
//! /dev/null && pwd -P >| …`, a child of the Claude Code process. The shell
//! forks a second one of the same arguments for a command made of several
//! parts, so a shell whose parent is such a shell is part of its parent's
//! tree, not another shell. No other tool's shells were running when this was
//! written, so none are matched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ack::agent::{VARIABLE, is_id_char};
use crate::procs::{Place, Processes, args, cwd, environment, place_of, tree_cpu};
use crate::text::tilde;

/// A shell this old, whose whole tree has used less than IDLE_SHARE of a
/// core, and that no live Claude Code session waits on, is stuck on input or
/// a lock.
pub const IDLE_MINUTES: i64 = 20;
pub const IDLE_SHARE: f64 = 0.02;
/// A copy younger than this may be a command run twice on purpose.
const COPY_SECONDS: i64 = 60;
/// How much of a command a line shows.
const SHOWN: usize = 100;

/// The running agent shells and those of them that are junk, oldest first.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Shells {
    pub running: usize,
    pub junk: Vec<Junk>,
    /// The checkouts (`repo/side-N`) an agent shell that is not junk runs
    /// in, each once: an agent still works there.
    #[serde(default)]
    pub active: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Junk {
    pub pid: i32,
    /// The process that started the shell: the agent's host, so the coordinator
    /// can tell which agent left it.
    pub parent: i32,
    /// The builds agent that ran the command (its BUILDS_AGENT), when the
    /// shell's environment or its command text says. Every subagent of one
    /// Claude Code session shares the parent process, so only this tells which
    /// agent left the shell.
    pub agent: Option<String>,
    pub minutes: i64,
    /// The checkout the command runs in, else its folder.
    pub worktree: String,
    pub reason: Reason,
    /// The command, without the tool's wrapper or a leading `cd`.
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reason {
    /// A process running under it is a wait, or its command is a wait loop;
    /// says which.
    Waits(String),
    /// A `find` or `du` over `/` or the home folder; says which.
    WholeDisk(String),
    /// The same command runs in the same worktree in an older shell.
    Duplicate { of: i32 },
    /// Used this share of a core over its life, as a percentage, and its
    /// session has ended: nothing waits on it.
    Idle { percent: f64 },
}

impl Reason {
    fn describe(&self) -> String {
        match self {
            Reason::Waits(by) => format!("waits ({by})"),
            Reason::WholeDisk(by) => format!("whole disk ({by})"),
            Reason::Duplicate { of } => format!("duplicate of pid {of}"),
            Reason::Idle { percent } => format!("idle ({percent:.1}% of a core since it started)"),
        }
    }
}

/// A process as the judgement reads it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub pid: i32,
    pub ppid: i32,
    /// Seconds since 1970.
    pub started: i64,
    /// CPU time the process and the children it has waited for used.
    pub cpu_seconds: f64,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// The process's BUILDS_AGENT, when its environment is readable and sets
    /// one. macOS hides the environment of Apple's own programs, so a shell
    /// or `sleep` never has one there.
    pub agent: Option<String>,
}

/// The value of BUILDS_AGENT in <environment>.
fn agent_in_environment(environment: &[String]) -> Option<String> {
    environment
        .iter()
        .find_map(|var| var.strip_prefix(VARIABLE)?.strip_prefix('='))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// The agent id a command's text sets: the first `BUILDS_AGENT=<id>`, as
/// `export BUILDS_AGENT=<id>` or `BUILDS_AGENT=<id> command` does.
fn agent_in_text(text: &str) -> Option<String> {
    let name = format!("{VARIABLE}=");
    let mut from = 0;
    while let Some(at) = text[from..].find(&name) {
        let start = from + at + name.len();
        let id: String = text[start..]
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| is_id_char(*c))
            .collect();
        // `XBUILDS_AGENT=` is another variable.
        let named = text[..from + at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if named && !id.is_empty() {
            return Some(id);
        }
        from = start;
    }
    None
}

fn is_agent_shell(entry: &Entry) -> bool {
    is_agent_argv(&entry.argv)
}

pub fn is_agent_argv(argv: &[String]) -> bool {
    let name = argv.first().map(|a| basename(a)).unwrap_or_default();
    matches!(name, "zsh" | "bash" | "sh") && argv.iter().any(|arg| arg.contains("shell-snapshots"))
}

pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The agent shells of <uid> and everything under them, read now.
pub fn gather(processes: &Processes, uid: u32) -> Vec<Entry> {
    let read = |pid: i32, ppid: i32, started: i64| {
        args(pid).map(|argv| Entry {
            pid,
            ppid,
            started,
            cpu_seconds: tree_cpu(pid).unwrap_or(0.0),
            argv,
            cwd: cwd(pid),
            agent: environment(pid).and_then(|env| agent_in_environment(&env)),
        })
    };
    let mut children: HashMap<i32, Vec<usize>> = HashMap::new();
    for (at, process) in processes.list.iter().enumerate() {
        children.entry(process.ppid).or_default().push(at);
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut seen = HashSet::new();
    let mut agent_shells = HashSet::new();
    for process in &processes.list {
        if process.uid == uid
            && matches!(process.name.as_str(), "zsh" | "bash" | "sh")
            && let Some(entry) = read(process.pid, process.ppid, process.started)
            && is_agent_shell(&entry)
        {
            agent_shells.insert(process.pid);
        }
    }
    for process in &processes.list {
        // The top of a tree: an agent shell whose parent is not one.
        if !agent_shells.contains(&process.pid) || agent_shells.contains(&process.ppid) {
            continue;
        }
        let mut pending = vec![process];
        while let Some(at) = pending.pop() {
            if !seen.insert(at.pid) {
                continue;
            }
            if let Some(entry) = read(at.pid, at.ppid, at.started) {
                entries.push(entry);
            }
            for child in children.get(&at.pid).into_iter().flatten() {
                pending.push(&processes.list[*child]);
            }
        }
    }
    entries
}

/// The command an agent shell runs: what its `eval '...'` holds, with the
/// quotes the tool added undone. The whole argument line when there is none.
pub fn command_of(argv: &[String]) -> String {
    let line = argv.join(" ");
    match line.find("eval '") {
        Some(at) => {
            let rest = &line[at + "eval '".len()..];
            let end = rest
                .rfind("' && pwd -P")
                .or_else(|| rest.rfind("' < /dev/null"))
                .unwrap_or(rest.len());
            rest[..end].replace("'\"'\"'", "'")
        }
        None => line,
    }
}

pub fn shown(text: &str) -> String {
    if text.chars().count() > SHOWN {
        format!("{}…", text.chars().take(SHOWN).collect::<String>())
    } else {
        text.to_string()
    }
}

/// Is <command> `builds dashboard`, which runs until it is stopped on
/// purpose?
pub fn runs_the_dashboard(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.windows(2).next().is_some_and(|pair| {
        (pair[0] == "builds" || pair[0].ends_with("/builds")) && pair[1] == "dashboard"
    })
}

/// A leading `cd <dir> &&` (or `;`) of <command>: the folder and the rest.
fn strip_cd(command: &str) -> (Option<String>, &str) {
    let trimmed = command.trim_start();
    let Some(rest) = trimmed.strip_prefix("cd ") else {
        return (None, trimmed);
    };
    let rest = rest.trim_start();
    let (dir, after) = match rest.chars().next() {
        Some(quote @ ('\'' | '"')) => match rest[1..].find(quote) {
            Some(end) => (&rest[1..1 + end], &rest[end + 2..]),
            None => return (None, trimmed),
        },
        _ => match rest.find(char::is_whitespace) {
            Some(end) => (&rest[..end], &rest[end..]),
            None => return (None, trimmed),
        },
    };
    let after = after.trim_start();
    match after.strip_prefix("&&").or_else(|| after.strip_prefix(';')) {
        Some(rest) => (Some(dir.to_string()), rest.trim_start()),
        None => (None, trimmed),
    }
}

/// The folder a `cd` names, against the shell's <cwd>.
fn resolve(dir: &str, cwd: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let dir = dir
        .strip_prefix("${HOME}")
        .or_else(|| dir.strip_prefix("$HOME"))
        .map(|rest| format!("~{rest}"))
        .unwrap_or(dir.to_string());
    if dir == "~" {
        Some(home.to_path_buf())
    } else if let Some(rest) = dir.strip_prefix("~/") {
        Some(home.join(rest))
    } else if dir.starts_with('/') {
        Some(PathBuf::from(dir))
    } else {
        cwd.map(|base| base.join(dir))
    }
}

/// <text> split into its simple commands, each a list of words, the way a
/// shell reads it: quotes keep a word whole, a heredoc's body and a comment
/// are not commands, and `;`, `&`, `|`, parentheses and newlines end one.
/// Leading `do`, `then`, `!`, `time` and `VAR=value` words are dropped.
fn commands(text: &str) -> Vec<Vec<String>> {
    struct Scan {
        out: Vec<Vec<String>>,
        command: Vec<String>,
        word: String,
        has_word: bool,
    }
    impl Scan {
        fn end_word(&mut self) {
            if self.has_word {
                self.command.push(std::mem::take(&mut self.word));
                self.has_word = false;
            }
        }
        fn end_command(&mut self) {
            self.end_word();
            let words = std::mem::take(&mut self.command);
            let skip = words.iter().take_while(|w| is_prefix(w)).count();
            if skip < words.len() {
                self.out.push(words[skip..].to_vec());
            }
        }
        fn push(&mut self, ch: char) {
            self.word.push(ch);
            self.has_word = true;
        }
    }
    fn is_prefix(word: &str) -> bool {
        matches!(
            word,
            "{" | "}"
                | "do"
                | "then"
                | "else"
                | "elif"
                | "if"
                | "!"
                | "time"
                | "exec"
                | "command"
                | "builtin"
                | "nohup"
        ) || word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        })
    }
    let c: Vec<char> = text.chars().collect();
    let mut scan = Scan {
        out: Vec::new(),
        command: Vec::new(),
        word: String::new(),
        has_word: false,
    };
    let mut heredocs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => {
                match c.get(i + 1) {
                    Some('\n') => scan.end_word(),
                    Some(next) => scan.push(*next),
                    None => {}
                }
                i += 2;
            }
            '\'' => {
                i += 1;
                while i < c.len() && c[i] != '\'' {
                    scan.push(c[i]);
                    i += 1;
                }
                scan.has_word = true;
                i += 1;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' && i + 1 < c.len() {
                        i += 1;
                    }
                    scan.push(c[i]);
                    i += 1;
                }
                scan.has_word = true;
                i += 1;
            }
            '#' if !scan.has_word => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '<' if c.get(i + 1) == Some(&'<') && c.get(i + 2) != Some(&'<') => {
                scan.end_word();
                i += 2;
                if c.get(i) == Some(&'-') {
                    i += 1;
                }
                while c.get(i).is_some_and(|ch| *ch == ' ' || *ch == '\t') {
                    i += 1;
                }
                let mut delimiter = String::new();
                while i < c.len() && !c[i].is_whitespace() && !";|&()<>".contains(c[i]) {
                    if !matches!(c[i], '\'' | '"' | '\\') {
                        delimiter.push(c[i]);
                    }
                    i += 1;
                }
                heredocs.push(delimiter);
            }
            '\n' => {
                scan.end_command();
                i += 1;
                for delimiter in heredocs.drain(..) {
                    while i < c.len() {
                        let end = c[i..]
                            .iter()
                            .position(|ch| *ch == '\n')
                            .map(|n| i + n)
                            .unwrap_or(c.len());
                        let line: String = c[i..end].iter().collect();
                        i = (end + 1).min(c.len());
                        if line.trim() == delimiter {
                            break;
                        }
                    }
                }
            }
            ' ' | '\t' => {
                scan.end_word();
                i += 1;
            }
            ';' | '&' | '|' | '(' | ')' | '`' => {
                scan.end_command();
                i += 1;
            }
            ch => {
                scan.push(ch);
                i += 1;
            }
        }
    }
    scan.end_command();
    scan.out
}

/// What a running process <argv> waits as, when it is a wait: `sleep`,
/// `caffeinate -t` or `-w`, `lsof +r`, python's `time.sleep`, perl's
/// `select(undef, undef, undef, N)` or `sleep`, ruby's `sleep`, and `timeout`
/// or `gtimeout` around any of those.
fn waits_as(argv: &[String]) -> Option<String> {
    let name = basename(argv.first()?);
    let rest = &argv[1..];
    match name {
        "sleep" => Some("sleep".to_string()),
        "caffeinate" if caffeinate_waits(rest) => Some("caffeinate -t/-w".to_string()),
        "lsof"
            if rest
                .iter()
                .any(|a| a.starts_with("+r") || a.starts_with("+R")) =>
        {
            Some("lsof +r".to_string())
        }
        n if n.starts_with("python") && rest.iter().any(|a| a.contains("time.sleep")) => {
            Some("time.sleep".to_string())
        }
        "perl" => {
            let script = one_liner(rest)?.replace(char::is_whitespace, "");
            if script.contains("select(undef,undef,undef,") {
                Some("perl select".to_string())
            } else if script.contains("sleep") {
                Some("perl sleep".to_string())
            } else {
                None
            }
        }
        "ruby" => one_liner(rest)?
            .contains("sleep")
            .then(|| "ruby sleep".to_string()),
        "timeout" | "gtimeout" => timeout_command(rest).and_then(waits_as),
        _ => None,
    }
}

/// The script of an interpreter's `-e` or `-E` option among <rest>.
fn one_liner(rest: &[String]) -> Option<&str> {
    let at = rest
        .iter()
        .position(|a| a.starts_with("-e") || a.starts_with("-E"))?;
    let flag = &rest[at];
    if flag.len() > 2 {
        Some(&flag[2..])
    } else {
        rest.get(at + 1).map(String::as_str)
    }
}

/// The command a `timeout` or `gtimeout` runs, from its arguments <rest>:
/// what follows its options and its duration.
fn timeout_command(rest: &[String]) -> Option<&[String]> {
    let mut i = 0;
    while let Some(word) = rest.get(i) {
        if word == "-s" || word == "-k" || word == "--signal" || word == "--kill-after" {
            i += 2;
        } else if word.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    rest.get(i + 1..).filter(|command| !command.is_empty())
}

/// Whether caffeinate with the arguments <rest> waits: `-t <seconds>` or
/// `-w <pid>` among its own options, which end at its program, and no program
/// to run. With a program caffeinate ignores both and runs the program, so
/// `caffeinate -i tar -tf a.tar` runs tar and waits on nothing.
fn caffeinate_waits(rest: &[String]) -> bool {
    let mut waits = false;
    let mut i = 0;
    while let Some(word) = rest.get(i).filter(|w| w.starts_with('-') && *w != "--") {
        let flags = &word[1..];
        if let Some(n) = flags.find(['t', 'w']) {
            waits = true;
            // The value is the rest of the word (`-w4242`), else the next word.
            if n == flags.len() - 1 {
                i += 1;
            }
        }
        i += 1;
    }
    if rest.get(i).is_some_and(|w| w == "--") {
        i += 1;
    }
    waits && i >= rest.len()
}

/// What a simple command <argv> of the shell's text loops as, when it starts
/// a wait loop: `until`, `while` other than `while read`, `read -t` (a read
/// that gives up after a timeout, which a `< <(sleep N)` feeds), or `timeout`
/// around a script that sleeps or reads with a timeout. A loop is a wait even
/// between two of its `sleep`s, when no `sleep` process is running.
fn loops_as(argv: &[String]) -> Option<String> {
    match basename(argv.first()?) {
        "read"
            if argv[1..]
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('t')) =>
        {
            Some("read -t".to_string())
        }
        "timeout" | "gtimeout" => (timeout_command(&argv[1..])
            .is_some_and(|c| waits_as(c).is_some())
            || argv[1..]
                .iter()
                .filter(|a| a.contains(char::is_whitespace))
                .flat_map(|script| commands(script))
                .any(|inner| {
                    inner.first().is_some_and(|w| basename(w) == "sleep")
                        || waits_as(&inner).is_some()
                        || loops_as(&inner).is_some()
                }))
        .then(|| "timeout around a sleep".to_string()),
        "until" => Some("an until loop".to_string()),
        "while"
            if !argv
                .get(1)
                .is_some_and(|a| a == "read" || a.starts_with("IFS")) =>
        {
            Some("a while loop".to_string())
        }
        _ => None,
    }
}

/// Whether the running process <argv> is a build or a test: `builds`
/// building, cargo, rustc, xcodebuild, gradle, swift.
fn runs_a_build(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    match basename(first) {
        "builds" => argv.get(1).is_some_and(|sub| {
            matches!(
                sub.as_str(),
                "app-build" | "build-app" | "cargo" | "land" | "checks"
            )
        }),
        "cargo" | "rustc" | "xcodebuild" | "gradle" | "gradlew" | "swift" | "swiftc"
        | "swift-build" | "swift-test" => true,
        _ => false,
    }
}

/// Whether <parsed> loops over a counted sequence: `for … in $(seq …)`.
fn loops_over_seq(parsed: &[Vec<String>]) -> bool {
    parsed.iter().any(|c| c.first().is_some_and(|w| w == "for"))
        && parsed.iter().any(|c| c.first().is_some_and(|w| w == "seq"))
}

/// What <argv> searches as, when it is a `find` or `du` over `/` or the home
/// folder.
fn whole_disk(argv: &[String], home: &Path) -> Option<String> {
    let name = basename(argv.first()?);
    let rest = &argv[1..];
    let operands: Vec<&String> = match name {
        "find" => {
            let skip = rest
                .iter()
                .take_while(|a| {
                    matches!(
                        a.as_str(),
                        "-H" | "-L" | "-P" | "-E" | "-X" | "-x" | "-d" | "-s"
                    )
                })
                .count();
            rest[skip..]
                .iter()
                .take_while(|a| !a.starts_with('-') && !a.starts_with('(') && !a.starts_with('!'))
                .collect()
        }
        "du" => rest.iter().filter(|a| !a.starts_with('-')).collect(),
        _ => return None,
    };
    let home = home.display().to_string();
    let wide = |path: &str| {
        let path = path.trim_end_matches('/');
        path.is_empty() || matches!(path, "~" | "$HOME" | "${HOME}" | "/Users") || path == home
    };
    operands
        .into_iter()
        .find(|operand| wide(operand))
        .map(|operand| format!("{name} {operand}"))
}

/// One agent shell as the judgement sees it.
struct Candidate {
    pid: i32,
    parent: i32,
    agent: Option<String>,
    started: i64,
    worktree: String,
    command: String,
    junk: Option<Reason>,
    cpu_seconds: f64,
    queued: bool,
    /// A live Claude Code session started the shell and waits on it. A
    /// command that waits for outside work, such as `gh run watch`, uses no
    /// CPU on purpose, so its CPU use says nothing.
    awaited: bool,
    /// A compiler runs under it: the compile may be in the sccache server's
    /// tree, so the shell's own CPU says nothing.
    compiling: bool,
    /// A `builds` command that builds runs in its tree (`runs_a_build`): one
    /// waiting for a lock or an app lane's queue uses next to no CPU and is
    /// not idle. A bare cargo blocked on a build-directory lock still is.
    building: bool,
}

/// Judges every agent shell in <entries> at <now>: which are junk, and why.
/// <queued> holds the pids of `builds` processes waiting in an app queue
/// (`Queue::waiting`): a shell with one in its tree waits on purpose.
/// <sessions> holds the pids of the running Claude Code processes
/// (`claude_sessions::pids`): a session waits on every shell it started, so a
/// shell whose parent is one is never idle.
pub fn judge(
    entries: &[Entry],
    now: i64,
    home: &Path,
    places: &[Place],
    queued: &HashSet<i32>,
    sessions: &HashSet<i32>,
) -> Shells {
    let mut children: HashMap<i32, Vec<&Entry>> = HashMap::new();
    for entry in entries {
        children.entry(entry.ppid).or_default().push(entry);
    }
    let tops: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            is_agent_shell(e) && !entries.iter().any(|p| p.pid == e.ppid && is_agent_shell(p))
        })
        .collect();
    let mut candidates = Vec::new();
    for top in &tops {
        let mut tree = Vec::new();
        let mut pending = vec![*top];
        while let Some(entry) = pending.pop() {
            tree.push(entry);
            pending.extend(children.get(&entry.pid).into_iter().flatten());
        }
        let command = command_of(&top.argv);
        let (dir, rest) = strip_cd(&command);
        let dir = match &dir {
            Some(dir) => resolve(dir, top.cwd.as_deref(), home),
            None => top.cwd.clone(),
        };
        let worktree = match dir {
            Some(dir) => match place_of(places, &dir) {
                Some(at) => places[at].label.clone(),
                None => tilde(&dir, home),
            },
            None => "unknown folder".to_string(),
        };
        if runs_the_dashboard(rest) {
            continue;
        }
        let parsed = commands(&command);
        let members: Vec<&Entry> = tree
            .iter()
            .filter(|e| !is_agent_shell(e))
            .copied()
            .collect();
        let argvs: Vec<&Vec<String>> = parsed
            .iter()
            .chain(members.iter().map(|e| &e.argv))
            .collect();
        // The text of a command says what it might run; a process says what
        // runs now. A `sleep` in the text may have finished, so only a
        // running process, or a loop around one, is a wait, and never while
        // a build or test runs in the tree.
        let building = members.iter().any(|e| runs_a_build(&e.argv));
        let junk = (!building)
            .then(|| {
                members
                    .iter()
                    .find_map(|e| waits_as(&e.argv))
                    .or_else(|| parsed.iter().find_map(|argv| loops_as(argv)))
                    .or_else(|| loops_over_seq(&parsed).then(|| "a for loop over seq".to_string()))
                    .map(Reason::Waits)
            })
            .flatten()
            .or_else(|| {
                argvs
                    .iter()
                    .find_map(|argv| whole_disk(argv, home).map(Reason::WholeDisk))
            });
        let agent = tree
            .iter()
            .find_map(|e| e.agent.clone())
            .or_else(|| agent_in_text(&command));
        candidates.push(Candidate {
            pid: top.pid,
            parent: top.ppid,
            agent,
            started: top.started,
            worktree,
            command: crate::text::one_line(rest),
            junk,
            cpu_seconds: tree.iter().map(|e| e.cpu_seconds).sum(),
            queued: tree.iter().any(|e| queued.contains(&e.pid)),
            awaited: sessions.contains(&top.ppid),
            building: members.iter().any(|e| {
                e.argv.first().is_some_and(|a| basename(a) == "builds") && runs_a_build(&e.argv)
            }),
            compiling: members.iter().any(|e| {
                e.argv
                    .first()
                    .is_some_and(|a| matches!(basename(a), "sccache" | "rustc"))
            }),
        });
    }
    let running = tops.len();
    for at in 0..candidates.len() {
        if candidates[at].junk.is_some() || candidates[at].command.is_empty() {
            continue;
        }
        let this = &candidates[at];
        let oldest = candidates
            .iter()
            .filter(|other| other.worktree == this.worktree && other.command == this.command)
            .min_by_key(|other| (other.started, other.pid))
            .expect("the shell is among its copies");
        let age = now - this.started;
        let junk = if oldest.pid != this.pid && age >= COPY_SECONDS {
            Some(Reason::Duplicate { of: oldest.pid })
        } else if age >= IDLE_MINUTES * 60
            && !this.queued
            && !this.awaited
            && !this.compiling
            && !this.building
            && this.cpu_seconds / (age as f64) < IDLE_SHARE
        {
            Some(Reason::Idle {
                percent: this.cpu_seconds / (age as f64) * 100.0,
            })
        } else {
            None
        };
        candidates[at].junk = junk;
    }
    let mut active: Vec<String> = Vec::new();
    for c in candidates.iter().filter(|c| c.junk.is_none()) {
        if !active.contains(&c.worktree) {
            active.push(c.worktree.clone());
        }
    }
    let mut junk: Vec<Junk> = candidates
        .into_iter()
        .filter_map(|c| {
            c.junk.map(|reason| Junk {
                pid: c.pid,
                parent: c.parent,
                agent: c.agent,
                minutes: crate::text::minutes_since(now, c.started),
                worktree: c.worktree,
                reason,
                command: shown(&c.command),
            })
        })
        .collect();
    junk.sort_by_key(|j| (std::cmp::Reverse(j.minutes), j.pid));
    Shells {
        running,
        junk,
        active,
    }
}

/// The lines `builds health` prints for <shells>.
pub fn lines(shells: &Shells) -> Vec<String> {
    let mut out = vec![format!(
        "shells: {} running, {} junk",
        shells.running,
        shells.junk.len()
    )];
    for j in &shells.junk {
        let age = if j.minutes >= 60 {
            format!("{}h{:02}m", j.minutes / 60, j.minutes % 60)
        } else {
            format!("{}m", j.minutes)
        };
        out.push(format!(
            "    pid {} ({}) {age}  {}  {}: {}",
            j.pid,
            origin(j),
            j.worktree,
            j.reason.describe(),
            j.command
        ));
    }
    out
}

/// Who left <junk>: its agent, when known, and the process that started it.
fn origin(junk: &Junk) -> String {
    let agent = junk.agent.as_deref().unwrap_or("unknown");
    format!("agent {agent}, started by pid {}", junk.parent)
}

/// The pids of <top> and everything under it, deepest first, from <pairs> of
/// (pid, parent pid).
fn tree_of(pairs: &[(i32, i32)], top: i32) -> Vec<i32> {
    let mut order = vec![top];
    let mut at = 0;
    while at < order.len() {
        let parent = order[at];
        for (pid, ppid) in pairs {
            if *ppid == parent && !order.contains(pid) {
                order.push(*pid);
            }
        }
        at += 1;
    }
    order.reverse();
    order
}

/// The line that says <junk> was stopped, with <count> processes.
fn stopped_line(junk: &Junk, count: usize) -> String {
    format!(
        "stopped pid {} and {} processes under it ({}, {}): {}: {}",
        junk.pid,
        count - 1,
        origin(junk),
        junk.worktree,
        junk.reason.describe(),
        junk.command
    )
}

/// Stops the whole tree of each junk shell in <shells> with SIGTERM, by PID,
/// deepest process first. A pid that is no longer an agent shell of <uid>
/// (it exited and the number went to another process) is left alone. Returns
/// one line per shell and the number of shells it could not stop.
pub fn stop(shells: &Shells, uid: u32) -> crate::fail::Result<(Vec<String>, usize)> {
    let processes = Processes::read()?;
    let pairs: Vec<(i32, i32)> = processes.list.iter().map(|p| (p.pid, p.ppid)).collect();
    let mut lines = Vec::new();
    let mut failed = 0;
    for junk in &shells.junk {
        let still_there = processes
            .get(junk.pid)
            .is_some_and(|p| p.uid == uid && args(junk.pid).is_some_and(|a| is_agent_argv(&a)));
        if !still_there {
            lines.push(format!(
                "pid {} is gone or no longer an agent shell; left alone",
                junk.pid
            ));
            continue;
        }
        let tree = tree_of(&pairs, junk.pid);
        let mut errors = Vec::new();
        for pid in &tree {
            if unsafe { libc::kill(*pid, libc::SIGTERM) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    errors.push(format!("pid {pid}: {error}"));
                }
            }
        }
        if errors.is_empty() {
            lines.push(stopped_line(junk, tree.len()));
        } else {
            failed += 1;
            lines.push(format!(
                "could not stop all of pid {}'s tree: {}",
                junk.pid,
                errors.join("; ")
            ));
        }
    }
    Ok((lines, failed))
}

#[cfg(test)]
#[path = "shells_tests.rs"]
mod tests;
