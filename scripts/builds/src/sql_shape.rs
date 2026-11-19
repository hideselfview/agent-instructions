//! The shape of SQL text: the text with its comments removed and its spacing
//! made even, so two texts that differ only in comments or layout have the
//! same shape. db.rs stamps a schema, and compares a store's tables with a
//! fresh store's, by shape.

/// <sql> with every comment (`-- ...` to the end of the line, and `/* ... */`)
/// replaced by one space, every run of whitespace outside a quoted name or
/// string made one space, and the ends trimmed. A quote (`'`, `"`, `` ` ``, or
/// `[` through `]`) holds its text as written, comment marks and spacing
/// included.
pub fn shape(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut at = 0;
    let mut space = false;
    while at < chars.len() {
        let c = chars[at];
        if c == '-' && chars.get(at + 1) == Some(&'-') {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
            space = true;
        } else if c == '/' && chars.get(at + 1) == Some(&'*') {
            at += 2;
            while at < chars.len() && !(chars[at] == '*' && chars.get(at + 1) == Some(&'/')) {
                at += 1;
            }
            at = (at + 2).min(chars.len());
            space = true;
        } else if c.is_whitespace() {
            at += 1;
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            let close = match c {
                '\'' | '"' | '`' => Some(c),
                '[' => Some(']'),
                _ => None,
            };
            out.push(c);
            at += 1;
            if let Some(close) = close {
                // A doubled quote inside a quote is one quote character.
                while at < chars.len() {
                    out.push(chars[at]);
                    at += 1;
                    if chars[at - 1] == close {
                        if close != ']' && chars.get(at) == Some(&close) {
                            out.push(close);
                            at += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "sql_shape_tests.rs"]
mod tests;
