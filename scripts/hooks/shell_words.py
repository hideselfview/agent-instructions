"""A Bash command as the shell splits it, for the PreToolUse hooks: the
commands it runs, each as its words, with quoted text (a commit message, a
grep pattern) kept as one word and never read as a separator."""

import re
import shlex

SEPARATOR = re.compile(r"[;&|()\n]+")


def unquoted_newlines_as_separators(command: str) -> str:
    # shlex reads a newline as whitespace; outside quotes it ends a command.
    out, quote, escaped = [], None, False
    for ch in command:
        if escaped:
            escaped = False
        elif ch == "\\" and quote != "'":
            escaped = True
        elif quote:
            if ch == quote:
                quote = None
        elif ch in "'\"":
            quote = ch
        elif ch == "\n":
            ch = ";"
        out.append(ch)
    return "".join(out)


def tokens(command: str):
    """Words and separators (`;`, `&&`, `|`, `(` ...), quotes removed."""
    lexer = shlex.shlex(unquoted_newlines_as_separators(command), posix=True, punctuation_chars=";&|()")
    lexer.whitespace_split = True
    lexer.commenters = "#"
    try:
        return list(lexer)
    except ValueError:
        return command.split()


def commands(command: str):
    """Each command's words, with the separator that ended it (None at the end)."""
    current = []
    for token in tokens(command):
        if SEPARATOR.fullmatch(token):
            yield current, token
            current = []
        else:
            current.append(token)
    yield current, None
