//! The shape of SQL text: the text with its comments removed and its spacing
//! made even, so two texts that differ only in comments or layout have the
//! same shape. db.rs stamps a schema, and compares a store's tables with a
//! fresh store's, by shape.
//!
//! `next`, a const function, writes the shape one byte at a time, so db.rs
//! computes the schema's stamp while the program compiles, and `shape`
//! collects the same bytes into a string at run time. Every mark the shape
//! reads (quotes, comment marks, ASCII spacing) is one byte, and no byte of a
//! character of more than one byte is one of them, so each character's bytes
//! pass through whole.

/// <sql> with every comment (`-- ...` to the end of the line, and `/* ... */`)
/// replaced by one space, every run of whitespace outside a quoted name or
/// string made one space, and the ends trimmed. A quote (`'`, `"`, `` ` ``, or
/// `[` through `]`) holds its text as written, comment marks and spacing
/// included.
pub fn shape(sql: &str) -> String {
    let mut out = Vec::with_capacity(sql.len());
    let mut reader = Reader::START;
    while let Some((byte, after)) = next(sql.as_bytes(), reader) {
        out.push(byte);
        reader = after;
    }
    String::from_utf8(out).expect("the shape copies each character's bytes whole")
}

/// Where the shape of a text stands: the byte it reads next, whether a space
/// is owed before the next byte it writes, the quote it is inside (its closing
/// byte, or 0), the second byte of a doubled closing quote it still owes, and
/// whether it has written a byte yet.
#[derive(Clone, Copy)]
pub struct Reader {
    at: usize,
    space: bool,
    quote: u8,
    owed: u8,
    wrote: bool,
}

impl Reader {
    /// The start of a text.
    pub const START: Reader = Reader {
        at: 0,
        space: false,
        quote: 0,
        owed: 0,
        wrote: false,
    };
}

/// Whether <byte> is spacing outside a quote: the ASCII whitespace, vertical
/// tab included.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The next byte of <sql>'s shape after <reader>, with where the shape stands
/// after it; None at the end.
pub const fn next(sql: &[u8], reader: Reader) -> Option<(u8, Reader)> {
    let mut r = reader;
    if r.owed != 0 {
        let byte = r.owed;
        r.owed = 0;
        return Some((byte, r));
    }
    let len = sql.len();
    if r.quote != 0 {
        if r.at >= len {
            return None;
        }
        let byte = sql[r.at];
        r.at += 1;
        if byte == r.quote {
            // A doubled quote inside a quote is one quote character.
            if r.quote != b']' && r.at < len && sql[r.at] == r.quote {
                r.at += 1;
                r.owed = byte;
            } else {
                r.quote = 0;
            }
        }
        return Some((byte, r));
    }
    while r.at < len {
        let byte = sql[r.at];
        let following = if r.at + 1 < len { sql[r.at + 1] } else { 0 };
        if byte == b'-' && following == b'-' {
            while r.at < len && sql[r.at] != b'\n' {
                r.at += 1;
            }
            r.space = true;
        } else if byte == b'/' && following == b'*' {
            r.at += 2;
            while r.at < len && !(sql[r.at] == b'*' && r.at + 1 < len && sql[r.at + 1] == b'/') {
                r.at += 1;
            }
            r.at = if r.at + 2 < len { r.at + 2 } else { len };
            r.space = true;
        } else if is_space(byte) {
            r.at += 1;
            r.space = true;
        } else {
            if r.space && r.wrote {
                r.space = false;
                return Some((b' ', r));
            }
            r.space = false;
            r.wrote = true;
            r.at += 1;
            r.quote = match byte {
                b'\'' | b'"' | b'`' => byte,
                b'[' => b']',
                _ => 0,
            };
            return Some((byte, r));
        }
    }
    None
}

#[cfg(test)]
#[path = "sql_shape_tests.rs"]
mod tests;
