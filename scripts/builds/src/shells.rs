//! Whether the shells that agents' tools started are doing real work. Many
//! agents run commands at once, each in a shell its tool starts; some of those
//! are junk: a wait loop, a search over the whole disk, a copy of a command
//! already running in the same worktree, a command stuck on input or a lock.
//! `builds health` lists them; it kills nothing, the coordinator does, by PID.
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

use crate::procs::{Place, Processes, args, cwd, place_of, tree_cpu};
use crate::text::tilde;

/// A shell this old, whose whole tree has used less than IDLE_SHARE of a
/// core, is stuck on input or a lock.
const IDLE_MINUTES: i64 = 20;
const IDLE_SHARE: f64 = 0.02;
/// A copy younger than this may be a command run twice on purpose.
const COPY_SECONDS: i64 = 60;
/// How much of a command a line shows.
const SHOWN: usize = 100;

/// The running agent shells and those of them that are junk, oldest first.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Shells {
    pub running: usize,
    pub junk: Vec<Junk>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Junk {
    pub pid: i32,
    pub minutes: i64,
    /// The checkout the command runs in, else its folder.
    pub worktree: String,
    pub reason: Reason,
    /// The command, without the tool's wrapper or a leading `cd`.
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reason {
    /// The command or a process under it is a wait; says which.
    Waits(String),
    /// A `find` or `du` over `/` or the home folder; says which.
    WholeDisk(String),
    /// The same command runs in the same worktree in an older shell.
    Duplicate { of: i32 },
    /// Used this share of a core over its life, as a percentage.
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
}

fn is_agent_shell(entry: &Entry) -> bool {
    let name = entry.argv.first().map(|a| basename(a)).unwrap_or_default();
    matches!(name, "zsh" | "bash" | "sh") && entry.argv.iter().any(|arg| arg.contains("shell-snapshots"))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The agent shells of <uid> and everything under them, read now.
pub fn gather(processes: &Processes, uid: u32) -> Vec<Entry> {
    let read = |pid: i32, ppid: i32, started: i64| {
        args(pid).map(|argv| Entry { pid, ppid, started, cpu_seconds: tree_cpu(pid).unwrap_or(0.0), argv, cwd: cwd(pid) })
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
fn command_of(argv: &[String]) -> String {
    let line = argv.join(" ");
    match line.find("eval '") {
        Some(at) => {
            let rest = &line[at + "eval '".len()..];
            let end = rest.rfind("' && pwd -P").or_else(|| rest.rfind("' < /dev/null")).unwrap_or(rest.len());
            rest[..end].replace("'\"'\"'", "'")
        }
        None => line,
    }
}

/// <text> on one line.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shown(text: &str) -> String {
    if text.chars().count() > SHOWN { format!("{}…", text.chars().take(SHOWN).collect::<String>()) } else { text.to_string() }
}

/// Is <command> `builds dashboard`, which runs until it is stopped on
/// purpose?
pub fn runs_the_dashboard(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.windows(2).next().is_some_and(|pair| (pair[0] == "builds" || pair[0].ends_with("/builds")) && pair[1] == "dashboard")
}

/// A leading `cd <dir> &&` (or `;`) of <command>: the folder and the rest.
fn strip_cd(command: &str) -> (Option<String>, &str) {
    let trimmed = command.trim_start();
    let Some(rest) = trimmed.strip_prefix("cd ") else { return (None, trimmed) };
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
    let dir = dir.strip_prefix("${HOME}").or_else(|| dir.strip_prefix("$HOME")).map(|rest| format!("~{rest}")).unwrap_or(dir.to_string());
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
        matches!(word, "{" | "}" | "do" | "then" | "else" | "elif" | "if" | "!" | "time" | "exec" | "command" | "builtin" | "nohup")
            || word.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
            })
    }
    let c: Vec<char> = text.chars().collect();
    let mut scan = Scan { out: Vec::new(), command: Vec::new(), word: String::new(), has_word: false };
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
                        let end = c[i..].iter().position(|ch| *ch == '\n').map(|n| i + n).unwrap_or(c.len());
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

/// What <argv> waits as, when it is a wait: `sleep`, `caffeinate -t` or
/// `-w`, `lsof +r`, python's `time.sleep`, an `until` or `while` loop. A
/// `while read` consumes input and is not one.
fn waits_as(argv: &[String]) -> Option<String> {
    let name = basename(argv.first()?);
    let rest = &argv[1..];
    match name {
        "sleep" => Some("sleep".to_string()),
        "caffeinate" if rest.iter().any(|a| a.starts_with('-') && a.chars().skip(1).any(|c| c == 't' || c == 'w')) => {
            Some("caffeinate -t/-w".to_string())
        }
        "lsof" if rest.iter().any(|a| a.starts_with("+r") || a.starts_with("+R")) => Some("lsof +r".to_string()),
        n if n.starts_with("python") && rest.iter().any(|a| a.contains("time.sleep")) => Some("time.sleep".to_string()),
        "until" => Some("an until loop".to_string()),
        "while" if !rest.first().is_some_and(|a| a == "read" || a.starts_with("IFS")) => Some("a while loop".to_string()),
        _ => None,
    }
}

/// Whether <parsed> loops over a counted sequence: `for … in $(seq …)`.
fn loops_over_seq(parsed: &[Vec<String>]) -> bool {
    parsed.iter().any(|c| c.first().is_some_and(|w| w == "for")) && parsed.iter().any(|c| c.first().is_some_and(|w| w == "seq"))
}

/// What <argv> searches as, when it is a `find` or `du` over `/` or the home
/// folder.
fn whole_disk(argv: &[String], home: &Path) -> Option<String> {
    let name = basename(argv.first()?);
    let rest = &argv[1..];
    let operands: Vec<&String> = match name {
        "find" => {
            let skip = rest.iter().take_while(|a| matches!(a.as_str(), "-H" | "-L" | "-P" | "-E" | "-X" | "-x" | "-d" | "-s")).count();
            rest[skip..].iter().take_while(|a| !a.starts_with('-') && !a.starts_with('(') && !a.starts_with('!')).collect()
        }
        "du" => rest.iter().filter(|a| !a.starts_with('-')).collect(),
        _ => return None,
    };
    let home = home.display().to_string();
    let wide = |path: &str| {
        let path = path.trim_end_matches('/');
        path.is_empty() || matches!(path, "~" | "$HOME" | "${HOME}" | "/Users") || path == home
    };
    operands.into_iter().find(|operand| wide(operand)).map(|operand| format!("{name} {operand}"))
}

/// One agent shell as the judgement sees it.
struct Candidate {
    pid: i32,
    started: i64,
    worktree: String,
    command: String,
    junk: Option<Reason>,
    cpu_seconds: f64,
    queued: bool,
    /// A compiler runs under it: the compile may be in the sccache server's
    /// tree, so the shell's own CPU says nothing.
    compiling: bool,
}

/// Judges every agent shell in <entries> at <now>: which are junk, and why.
/// <queued> holds the pids of `builds` processes waiting in an app queue
/// (`Queue::waiting`): a shell with one in its tree waits on purpose.
pub fn judge(entries: &[Entry], now: i64, home: &Path, places: &[Place], queued: &HashSet<i32>) -> Shells {
    let mut children: HashMap<i32, Vec<&Entry>> = HashMap::new();
    for entry in entries {
        children.entry(entry.ppid).or_default().push(entry);
    }
    let tops: Vec<&Entry> =
        entries.iter().filter(|e| is_agent_shell(e) && !entries.iter().any(|p| p.pid == e.ppid && is_agent_shell(p))).collect();
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
        let members: Vec<&Entry> = tree.iter().filter(|e| !is_agent_shell(e)).copied().collect();
        let argvs: Vec<&Vec<String>> = parsed.iter().chain(members.iter().map(|e| &e.argv)).collect();
        let junk = argvs
            .iter()
            .find_map(|argv| waits_as(argv).map(Reason::Waits))
            .or_else(|| loops_over_seq(&parsed).then(|| Reason::Waits("a for loop over seq".to_string())))
            .or_else(|| argvs.iter().find_map(|argv| whole_disk(argv, home).map(Reason::WholeDisk)));
        candidates.push(Candidate {
            pid: top.pid,
            started: top.started,
            worktree,
            command: flat(rest),
            junk,
            cpu_seconds: tree.iter().map(|e| e.cpu_seconds).sum(),
            queued: tree.iter().any(|e| queued.contains(&e.pid)),
            compiling: members.iter().any(|e| e.argv.first().is_some_and(|a| matches!(basename(a), "sccache" | "rustc"))),
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
        } else if age >= IDLE_MINUTES * 60 && !this.queued && !this.compiling && this.cpu_seconds / (age as f64) < IDLE_SHARE {
            Some(Reason::Idle { percent: this.cpu_seconds / (age as f64) * 100.0 })
        } else {
            None
        };
        candidates[at].junk = junk;
    }
    let mut junk: Vec<Junk> = candidates
        .into_iter()
        .filter_map(|c| {
            c.junk.map(|reason| Junk {
                pid: c.pid,
                minutes: (now - c.started).max(0) / 60,
                worktree: c.worktree,
                reason,
                command: shown(&c.command),
            })
        })
        .collect();
    junk.sort_by_key(|j| (std::cmp::Reverse(j.minutes), j.pid));
    Shells { running, junk }
}

/// The lines `builds health` prints for <shells>.
pub fn lines(shells: &Shells) -> Vec<String> {
    let mut out = vec![format!("shells: {} running, {} junk", shells.running, shells.junk.len())];
    for j in &shells.junk {
        let age = if j.minutes >= 60 { format!("{}h{:02}m", j.minutes / 60, j.minutes % 60) } else { format!("{}m", j.minutes) };
        out.push(format!("    pid {} {age}  {}  {}: {}", j.pid, j.worktree, j.reason.describe(), j.command));
    }
    out
}

#[cfg(test)]
#[path = "shells_tests.rs"]
mod tests;
