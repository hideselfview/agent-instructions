//! Which changes to a source file are only in its comments. A change is
//! comment-only when the file's text before and after differ, and are the
//! same once every comment is gone, read by a lexer of the file's language that knows its
//! string and character literals (so `"//"` inside a string is no comment)
//! and its nested block comments, and the change neither adds nor removes the
//! file. builds lexes Swift, Kotlin, C# and Rust, the languages of the app
//! sources whose change decides an app build; a file of any other language
//! changes code.
//!
//! Doc comments are comments. A change to whitespace outside comments is code,
//! because compilers and formatters read it, with one exception: the spaces
//! between a comment and the code before it on its line, and the line a
//! comment alone fills, go with the comment, so that adding or deleting a
//! comment whole is comment-only.

use std::path::Path;

use crate::fail::Result;

/// A language builds can lex comments out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Swift,
    Kotlin,
    CSharp,
    Rust,
}

impl Language {
    /// The language of the file at <path>, by its extension, or None when
    /// builds has no lexer for it.
    pub fn of(path: &str) -> Option<Language> {
        match Path::new(path).extension()?.to_str()? {
            "swift" => Some(Language::Swift),
            "kt" | "kts" => Some(Language::Kotlin),
            "cs" => Some(Language::CSharp),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    /// Whether a block comment inside a block comment opens a level that
    /// needs its own `*/`: in Swift, Kotlin and Rust, not in C#.
    fn nests_block_comments(self) -> bool {
        self != Language::CSharp
    }
}

/// Whether the change of the file at <path> from <before> to <after>, each
/// None when the file is absent, is only in comments. Two equal texts are no
/// change at all.
pub fn only_in_comments(path: &str, before: Option<&str>, after: Option<&str>) -> bool {
    let (Some(language), Some(before), Some(after)) = (Language::of(path), before, after) else {
        return false;
    };
    before != after && without_comments(language, before) == without_comments(language, after)
}

/// The paths of <paths> whose change from the commit or tree <from> to <to>
/// in the repository at <top> is only in comments (`only_in_comments`).
pub fn paths_only_in_comments(
    top: &Path,
    from: &str,
    to: &str,
    paths: &[String],
) -> Result<Vec<String>> {
    let mut only = Vec::new();
    for path in paths {
        if Language::of(path).is_none() {
            continue;
        }
        let before = text_at(top, from, path)?;
        let after = text_at(top, to, path)?;
        if only_in_comments(path, before.as_deref(), after.as_deref()) {
            only.push(path.clone());
        }
    }
    Ok(only)
}

/// The text of <path> in the commit or tree <rev> of the repository at
/// <top>, or None when <rev> has no file there (a submodule is no file).
fn text_at(top: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    let entry = crate::git::text(top, &["ls-tree", "-z", "--full-tree", rev, "--", path])?;
    let blob = entry.split('\0').find_map(|line| {
        let (meta, named) = line.split_once('\t')?;
        let mut fields = meta.split(' ');
        let (_, kind, sha) = (fields.next()?, fields.next()?, fields.next()?);
        (named == path && kind == "blob").then_some(sha)
    });
    match blob {
        Some(sha) => crate::git::text(top, &["cat-file", "blob", sha]).map(Some),
        None => Ok(None),
    }
}

/// <text> with its comments gone: each comment that ends its line goes with
/// the spaces before it, and with its line when nothing else is on it; each
/// other comment becomes one space between two pieces of code that would
/// otherwise touch.
pub fn without_comments(language: Language, text: &str) -> String {
    let mut lexer = Lexer {
        language,
        text: text.as_bytes(),
        at: 0,
        comments: Vec::new(),
    };
    lexer.code(None);
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (start, end) in lexer.comments {
        out.push_str(&text[from..start]);
        let line_end = text[end..].find('\n').map_or(text.len(), |at| end + at);
        let rest = &text[end..line_end];
        if rest
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
        {
            let kept = out.trim_end_matches([' ', '\t']).len();
            out.truncate(kept);
            from = if out.is_empty() || out.ends_with('\n') {
                (line_end + 1).min(text.len())
            } else {
                line_end - rest.len() + rest.trim_end_matches([' ', '\t']).len()
            };
        } else {
            let touches = |c: Option<char>| c.is_some_and(|c| !c.is_whitespace());
            if touches(out.chars().last()) && touches(text[end..].chars().next()) {
                out.push(' ');
            }
            from = end;
        }
    }
    out.push_str(&text[from..]);
    out
}

/// Reads code and records where each comment starts and ends, as byte
/// offsets. Every delimiter it reads is ASCII, so a byte of a multi-byte
/// character never matches one.
struct Lexer<'a> {
    language: Language,
    text: &'a [u8],
    at: usize,
    comments: Vec<(usize, usize)>,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.text.get(self.at + ahead).copied()
    }

    fn starts(&self, what: &[u8]) -> bool {
        self.text[self.at..].starts_with(what)
    }

    /// How many bytes equal to <byte> start at <at>.
    fn run_of(&self, at: usize, byte: u8) -> usize {
        self.text[at.min(self.text.len())..]
            .iter()
            .take_while(|b| **b == byte)
            .count()
    }

    /// Reads code up to the end of the text, or, with <close>, past the
    /// closing bracket that matches no opening one it read: the end of a
    /// string's interpolation.
    fn code(&mut self, close: Option<u8>) {
        let open = match close {
            Some(b')') => Some(b'('),
            Some(b'}') => Some(b'{'),
            _ => None,
        };
        let mut depth = 0usize;
        while let Some(byte) = self.peek(0) {
            if Some(byte) == open {
                depth += 1;
            } else if Some(byte) == close {
                if depth == 0 {
                    self.at += 1;
                    return;
                }
                depth -= 1;
            }
            if self.starts(b"//") {
                let start = self.at;
                while self.peek(0).is_some_and(|b| b != b'\n') {
                    self.at += 1;
                }
                self.comments.push((start, self.at));
            } else if self.starts(b"/*") {
                self.block_comment();
            } else if !self.literal() {
                self.at += 1;
            }
        }
    }

    fn block_comment(&mut self) {
        let start = self.at;
        self.at += 2;
        let mut depth = 1;
        while self.at < self.text.len() {
            if self.starts(b"*/") {
                self.at += 2;
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if self.language.nests_block_comments() && self.starts(b"/*") {
                self.at += 2;
                depth += 1;
            } else {
                self.at += 1;
            }
        }
        self.comments.push((start, self.at));
    }

    /// Reads the string or character literal that starts here, when one
    /// does, and says whether it read one.
    fn literal(&mut self) -> bool {
        match self.language {
            Language::Swift => self.swift_literal(),
            Language::Kotlin => self.kotlin_literal(),
            Language::CSharp => self.csharp_literal(),
            Language::Rust => self.rust_literal(),
        }
    }

    /// Whether the byte before <at> continues a word, so that a literal's
    /// prefix there (Rust's `r`, `b`) is the end of a name instead.
    fn after_word(&self, at: usize) -> bool {
        at > 0 && {
            let byte = self.text[at - 1];
            byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
        }
    }

    /// A character literal of Kotlin, C# or Rust from its opening `'`: to the
    /// next `'` that no backslash escapes.
    fn char_literal(&mut self) {
        self.at += 1;
        while let Some(byte) = self.peek(0) {
            self.at += 1;
            match byte {
                b'\\' => self.at += 1,
                b'\'' | b'\n' => return,
                _ => {}
            }
        }
    }

    /// Swift: `"…"` and `"""…"""`, each with escapes and `\(…)`
    /// interpolation, and the raw forms that put one or more `#` around them,
    /// whose escapes and interpolation take as many `#` after the backslash.
    fn swift_literal(&mut self) -> bool {
        let hashes = self.run_of(self.at, b'#');
        if self.text.get(self.at + hashes) != Some(&b'"') {
            return false;
        }
        self.at += hashes;
        let quotes = if self.starts(b"\"\"\"") { 3 } else { 1 };
        self.at += quotes;
        let mut end = vec![b'"'; quotes];
        end.extend(std::iter::repeat_n(b'#', hashes));
        while self.at < self.text.len() {
            if self.starts(&end) {
                self.at += end.len();
                return true;
            }
            if self.peek(0) == Some(b'\\') && self.run_of(self.at + 1, b'#') >= hashes {
                self.at += 1 + hashes;
                if self.peek(0) == Some(b'(') {
                    self.at += 1;
                    self.code(Some(b')'));
                } else {
                    self.at += 1;
                }
                continue;
            }
            if quotes == 1 && self.peek(0) == Some(b'\n') {
                return true;
            }
            self.at += 1;
        }
        true
    }

    /// Kotlin: `'c'`, `"…"` with escapes and `${…}` templates, and `"""…"""`
    /// with templates and no escapes, whose extra closing quotes belong to it.
    fn kotlin_literal(&mut self) -> bool {
        match self.peek(0) {
            Some(b'\'') => {
                self.char_literal();
                true
            }
            Some(b'"') => {
                let raw = self.starts(b"\"\"\"");
                self.at += if raw { 3 } else { 1 };
                while let Some(byte) = self.peek(0) {
                    if raw && self.starts(b"\"\"\"") {
                        self.at += self.run_of(self.at, b'"');
                        return true;
                    }
                    if !raw && byte == b'"' {
                        self.at += 1;
                        return true;
                    }
                    if self.starts(b"${") {
                        self.at += 2;
                        self.code(Some(b'}'));
                        continue;
                    }
                    self.at += if !raw && byte == b'\\' { 2 } else { 1 };
                }
                true
            }
            _ => false,
        }
    }

    /// C#: `'c'`; `"…"` with escapes; `@"…"` with `""` for a quote; either
    /// of those after `$`, with `{{` for a brace and `{…}` holes; and raw
    /// `"""…"""` with three or more quotes, closed by as many, after any
    /// number of `$`.
    fn csharp_literal(&mut self) -> bool {
        if self.peek(0) == Some(b'\'') {
            self.char_literal();
            return true;
        }
        let dollars = self.run_of(self.at, b'$');
        let mut at = self.at + dollars;
        let verbatim = self.text.get(at) == Some(&b'@');
        if verbatim {
            at += 1;
        }
        let dollars = dollars + self.run_of(at, b'$');
        at += self.run_of(at, b'$');
        if self.text.get(at) != Some(&b'"') {
            return false;
        }
        self.at = at;
        let quotes = self.run_of(self.at, b'"');
        if quotes >= 3 && !verbatim {
            self.at += quotes;
            let end = vec![b'"'; quotes];
            while self.at < self.text.len() && !self.starts(&end) {
                self.at += 1;
            }
            self.at = (self.at + quotes).min(self.text.len());
            return true;
        }
        self.at += 1;
        let holes = dollars > 0;
        while let Some(byte) = self.peek(0) {
            if byte == b'"' {
                if verbatim && self.peek(1) == Some(b'"') {
                    self.at += 2;
                    continue;
                }
                self.at += 1;
                return true;
            }
            if holes && self.starts(b"{{") {
                self.at += 2;
            } else if holes && byte == b'{' {
                self.at += 1;
                self.code(Some(b'}'));
            } else if !verbatim && byte == b'\\' {
                self.at += 2;
            } else if !verbatim && byte == b'\n' {
                return true;
            } else {
                self.at += 1;
            }
        }
        true
    }

    /// Rust: `"…"` with escapes (also after `b` or `c`), the raw `r"…"`,
    /// `r#"…"#` and their `b` and `c` forms, and `'c'`, `b'c'` and `'\n'`,
    /// told from a lifetime or label `'a` by the quote after one character.
    fn rust_literal(&mut self) -> bool {
        match self.peek(0) {
            Some(b'"') => {
                self.at += 1;
                while let Some(byte) = self.peek(0) {
                    self.at += 1;
                    match byte {
                        b'\\' => self.at += 1,
                        b'"' => return true,
                        _ => {}
                    }
                }
                true
            }
            Some(b'\'') => {
                let after = std::str::from_utf8(&self.text[self.at + 1..])
                    .ok()
                    .and_then(|rest| rest.chars().next());
                let closes = after
                    .is_some_and(|c| self.text.get(self.at + 1 + c.len_utf8()) == Some(&b'\''));
                if self.peek(1) == Some(b'\\') || closes {
                    self.char_literal();
                } else {
                    self.at += 1;
                }
                true
            }
            Some(b'r' | b'b' | b'c') if !self.after_word(self.at) => {
                let mut at = self.at;
                if matches!(self.text[at], b'b' | b'c') {
                    at += 1;
                }
                if self.text.get(at) != Some(&b'r') {
                    return false;
                }
                at += 1;
                let hashes = self.run_of(at, b'#');
                if self.text.get(at + hashes) != Some(&b'"') {
                    return false;
                }
                self.at = at + hashes + 1;
                let mut end = vec![b'"'];
                end.extend(std::iter::repeat_n(b'#', hashes));
                while self.at < self.text.len() && !self.starts(&end) {
                    self.at += 1;
                }
                self.at = (self.at + end.len()).min(self.text.len());
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "comments_tests.rs"]
mod tests;
