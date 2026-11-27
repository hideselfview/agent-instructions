//! The files cargo builds `builds` from, which build.rs hashes and keeps a
//! copy of, and which build.rs's `cargo:rerun-if-changed` lines name. build.rs
//! includes this file as a module, and the tests in src/build_source_tests.rs
//! include it too.
//!
//! The builds crate's folder is the one cargo builds (`CARGO_MANIFEST_DIR`),
//! whatever it is called: `builds` in a checkout, `src` in the folder that
//! install.sh builds from. The boards and shots crates are the folders of
//! those names beside it.

use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

/// The files and folders, relative to a crate's folder, that cargo builds
/// the crate from.
const PARTS: [&str; 6] = [
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "build_source.rs",
    "src",
    "runtime",
];

/// The files and folders, relative to each crate's folder in the scripts
/// folder, that the shots checker is built from (`shots_files`).
const SHOTS_PARTS: [(&str, &[&str]); 2] = [
    ("shots", &["Cargo.toml", "src", "canvas", "harness"]),
    ("boards", &["Cargo.toml", "src", "runtime"]),
];

/// The roots of the crate <name> in the folder <dir>: each of <parts>, as the
/// name its files carry in the hash and the copy, and its path. Fails when
/// the crate lacks its Cargo.toml or its src: a folder that holds neither is
/// not the crate, and a version that hashes nothing of it names every version
/// alike.
fn crate_roots(name: &str, dir: &Path, parts: &[&str]) -> Vec<(String, PathBuf)> {
    for part in ["Cargo.toml", "src"] {
        assert!(
            dir.join(part).exists(),
            "{} is not there: build.rs hashes {name}/{part} from it",
            dir.join(part).display()
        );
    }
    parts
        .iter()
        .map(|part| (format!("{name}/{part}"), dir.join(part)))
        .collect()
}

/// Each file or folder that builds is built from, as the name its files
/// carry in the hash and the copy, and the path: <here>, the builds crate's
/// folder, and the boards and shots crates in <scripts>.
pub fn roots(here: &Path, scripts: &Path) -> Vec<(String, PathBuf)> {
    let (shots, shots_parts) = SHOTS_PARTS[0];
    let mut roots = crate_roots("builds", here, &PARTS);
    roots.extend(crate_roots("boards", &scripts.join("boards"), &PARTS));
    roots.extend(crate_roots(shots, &scripts.join(shots), shots_parts));
    roots
}

/// Every file under <roots>, sorted: its shown name and its path.
pub fn files(roots: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for (shown, path) in roots {
        collect(path, shown, &mut files);
    }
    files.sort();
    files
}

/// A platform whose shots the checker takes with files of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Taker {
    Mac,
    Android,
    Web,
}

/// The files and folders of the shots crate that take one platform's shots
/// and nothing else, so that a change to them keeps what the other platforms'
/// runs and every board picture depend on.
const TAKERS: [(&str, Taker); 5] = [
    ("shots/harness/mac", Taker::Mac),
    ("shots/src/harness/mac.rs", Taker::Mac),
    ("shots/harness/android", Taker::Android),
    ("shots/src/harness/android.rs", Taker::Android),
    ("shots/src/capture/web.rs", Taker::Web),
];

/// The side of the shots checker a version covers: the files that draw a
/// board, those and the files that take one platform's shots, or the files
/// that compare a shot with its board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// What a cached board picture depends on: every file of the capture side
    /// but each platform's own (`TAKERS`).
    Boards,
    /// What a platform's cached shots and runs depend on: `Boards` and the
    /// platform's own files.
    Capture(Taker),
    /// What a cached run's findings depend on, given its shots and pictures.
    Comparison,
}

/// The files of the shots crate's `src` that both sides read: the command
/// line, the shot and element-list types, and the names, settings and
/// declarations that say which shots exist, and the backends that hold each
/// platform's facts.
const SHARED: [&str; 8] = [
    "backend.rs",
    "lib.rs",
    "shots.rs",
    "names.rs",
    "settings.rs",
    "declared.rs",
    "hex.rs",
    "log.rs",
];

/// The files and folders of the shots crate's `src` that only the capture
/// side reads: taking shots, drawing boards and keeping their pictures. The
/// harness sources beside `src` (`shots/harness`) take shots too, inside an
/// app's tests.
const CAPTURE_ONLY: [&str; 9] = [
    "capture",
    "harness",
    "harness.rs",
    "capture_marks.rs",
    "render",
    "render.rs",
    "design.rs",
    "board_sizes.rs",
    "pictures.rs",
];

/// The files and folders of the shots crate's `src` that only the comparison
/// side reads: comparing a pair, reading the run it compares with, and writing
/// the findings and the report. `verify.rs` also draws the boards it compares,
/// but the picture of a board is keyed by `design.rs`'s hash of everything it
/// depends on, so what it draws does not depend on `verify.rs`.
const COMPARISON_ONLY: [&str; 10] = [
    "compare",
    "compare.rs",
    "before.rs",
    "findings_file.rs",
    "recheck.rs",
    "report.rs",
    "verify.rs",
    "component_files.rs",
    "lint.rs",
    "lists.rs",
];

/// The files of the shots crate's `src` that neither side reads: `builds shots
/// states`, which lists the component states the boards draw, `builds shots
/// draws`, which says whether boards draw the same at two commits, what builds
/// asks outside a run (library.rs), and the command line (cli.rs): the
/// subcommands and arguments name no picture, so a change to one keeps every
/// stored shot, board picture and run. The picture of a web page
/// (web_picture.rs) is no shot and no board.
const UNVERSIONED: [&str; 5] = [
    "states.rs",
    "draws.rs",
    "library.rs",
    "cli.rs",
    "web_picture.rs",
];

/// Who reads a file of the shots checker.
#[derive(PartialEq, Eq, Debug)]
enum Readers {
    Both,
    /// Every platform's capture and the board pictures.
    Capture,
    /// One platform's capture.
    Takes(Taker),
    Comparison,
    Nobody,
}

/// Who reads <shown>, a file of the shots checker (`shots_files`). Fails for
/// a file of the shots crate's `src` that no list above names, so that a new
/// file says which side it belongs to. A test file (`_tests.rs`) has no readers:
/// it changes no run.
fn readers(shown: &str) -> Readers {
    if shown.starts_with("boards/") || shown == "shots/Cargo.toml" {
        return Readers::Both;
    }
    if shown.ends_with("_tests.rs") {
        return Readers::Nobody;
    }
    let is_at = |at: &str| {
        shown
            .strip_prefix(at)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    if let Some((_, taker)) = TAKERS.iter().find(|(at, _)| is_at(at)) {
        return Readers::Takes(*taker);
    }
    if shown.starts_with("shots/canvas/") {
        return Readers::Capture;
    }
    let Some(inside) = shown.strip_prefix("shots/src/") else {
        panic!("{shown} is no file of the shots checker that build_source.rs knows");
    };
    let top = inside.split('/').next().unwrap_or(inside);
    if SHARED.contains(&top) {
        Readers::Both
    } else if CAPTURE_ONLY.contains(&top) {
        Readers::Capture
    } else if COMPARISON_ONLY.contains(&top) {
        Readers::Comparison
    } else if UNVERSIONED.contains(&top) {
        Readers::Nobody
    } else {
        panic!(
            "{shown} is in no list of build_source.rs (SHARED, CAPTURE_ONLY, COMPARISON_ONLY, UNVERSIONED): say which side of the shots checker reads it"
        )
    }
}

/// Every file of the shots checker that <side> reads, sorted: its shown name
/// and its path, in the crates of <scripts>.
pub fn shots_files(scripts: &Path, side: Side) -> Vec<(String, PathBuf)> {
    let roots: Vec<_> = SHOTS_PARTS
        .iter()
        .flat_map(|(name, parts)| crate_roots(name, &scripts.join(name), parts))
        .collect();
    files(&roots)
        .into_iter()
        .filter(|(shown, _)| match readers(shown) {
            Readers::Both => true,
            Readers::Capture => side != Side::Comparison,
            Readers::Takes(taker) => side == Side::Capture(taker),
            Readers::Comparison => side == Side::Comparison,
            Readers::Nobody => false,
        })
        .collect()
}

/// What a version of the shots checker hashes of the file <shown>, whose
/// contents are <bytes>: the part of it that can change what the checker
/// does. Of a Rust file that is its tokens without comments, doc comments or
/// test modules. Of a Kotlin or Swift file it is every line but those that
/// hold only a `//` comment, unless the file opens a block comment or a
/// multi-line string, which such a line could be part of. Of any other file it
/// is the whole contents.
pub fn meaning(shown: &str, bytes: Vec<u8>) -> Vec<u8> {
    let text = |bytes: Vec<u8>| {
        String::from_utf8(bytes).unwrap_or_else(|error| panic!("{shown} is not UTF-8: {error}"))
    };
    if shown.ends_with(".rs") {
        let tokens: TokenStream = text(bytes)
            .parse()
            .unwrap_or_else(|error| panic!("{shown} is no Rust: {error}"));
        let mut meant = String::new();
        write_tokens(tokens, &mut meant);
        meant.into_bytes()
    } else if shown.ends_with(".kt") || shown.ends_with(".swift") {
        let source = text(bytes);
        if source.contains("/*") || source.contains("\"\"\"") {
            return source.into_bytes();
        }
        source
            .split_inclusive('\n')
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>()
            .into_bytes()
    } else {
        bytes
    }
}

/// Writes each token of <tokens> into <into> with a space after it, leaving
/// out each doc comment (which reads as a `doc` attribute) and each test
/// module.
fn write_tokens(tokens: TokenStream, into: &mut String) {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut at = 0;
    while at < tokens.len() {
        if let Some((attribute, length)) = attribute_at(&tokens, at)
            && attribute.starts_with("doc =")
        {
            at += length;
            continue;
        }
        if let Some(end) = test_module_end(&tokens, at) {
            at = end;
            continue;
        }
        match &tokens[at] {
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    Delimiter::Parenthesis => ("( ", ") "),
                    Delimiter::Brace => ("{ ", "} "),
                    Delimiter::Bracket => ("[ ", "] "),
                    Delimiter::None => ("", ""),
                };
                into.push_str(open);
                write_tokens(group.stream(), into);
                into.push_str(close);
            }
            // A joint punctuation mark and the one after it are one operator.
            TokenTree::Punct(punct) if punct.spacing() == Spacing::Joint => {
                into.push(punct.as_char());
            }
            token => {
                into.push_str(&token.to_string());
                into.push(' ');
            }
        }
        at += 1;
    }
}

/// The attribute that starts at <at> in <tokens>, as its text, with the number
/// of tokens it takes: `#[...]` or `#![...]`.
fn attribute_at(tokens: &[TokenTree], at: usize) -> Option<(String, usize)> {
    let is = |at: usize, mark: char| matches!(tokens.get(at), Some(TokenTree::Punct(punct)) if punct.as_char() == mark);
    if !is(at, '#') {
        return None;
    }
    let inner = usize::from(is(at + 1, '!'));
    match tokens.get(at + 1 + inner) {
        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket => {
            Some((group.stream().to_string(), inner + 2))
        }
        _ => None,
    }
}

/// Where the test module that starts at <at> in <tokens> ends: a
/// `#[cfg(test)]` attribute, any other attributes, and a `mod` with its `;` or
/// its braces. None when no test module starts there.
fn test_module_end(tokens: &[TokenTree], at: usize) -> Option<usize> {
    let (first, length) = attribute_at(tokens, at)?;
    if first.replace(' ', "") != "cfg(test)" {
        return None;
    }
    let mut next = at + length;
    while let Some((_, length)) = attribute_at(tokens, next) {
        next += length;
    }
    if !matches!(tokens.get(next), Some(TokenTree::Ident(word)) if word == "mod") {
        return None;
    }
    let ends = |token: &TokenTree| match token {
        TokenTree::Punct(punct) => punct.as_char() == ';',
        TokenTree::Group(group) => group.delimiter() == Delimiter::Brace,
        _ => false,
    };
    tokens[next..]
        .iter()
        .position(ends)
        .map(|end| next + end + 1)
}

/// Adds <path> (shown as <shown>), or every file under it when it is a
/// folder, to <files>. A part a crate lacks adds nothing; any other failure
/// to read a part stops the build.
fn collect(path: &Path, shown: &str, files: &mut Vec<(String, PathBuf)>) {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("reading {}: {error}", path.display()),
    };
    if meta.is_file() {
        files.push((shown.to_string(), path.to_path_buf()));
        return;
    }
    let entries = std::fs::read_dir(path)
        .unwrap_or_else(|error| panic!("listing {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("listing {}: {error}", path.display()));
        let name = entry.file_name().to_string_lossy().into_owned();
        collect(&entry.path(), &format!("{shown}/{name}"), files);
    }
}
