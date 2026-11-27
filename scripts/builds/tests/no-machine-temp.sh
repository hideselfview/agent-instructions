#!/bin/sh
# Runs a test where macOS refuses each write under the machine's temporary
# folders (.config/nextest.toml). ps is setuid, which a sandboxed process may
# not run, and writes nothing. Each of the Mac's developer tools, git among them,
# keeps its xcrun_db cache of tool paths in the user's temporary folder.
# Chrome keeps its socket folder in the user's temporary folder, whatever
# TMPDIR says, unless this names another.
export MAC_CHROMIUM_TMPDIR="${TMPDIR:?run the tests with builds cargo, which gives the run a temporary folder}"
user_temp=$(cd "$(getconf DARWIN_USER_TEMP_DIR)" && pwd -P) || exit 1
exec /usr/bin/sandbox-exec -D "USER_TEMP=$user_temp" -p '(version 1)
(allow default)
(deny file-write* (subpath "/private/tmp") (subpath (param "USER_TEMP")))
(allow file-write* (regex (string-append "^" (param "USER_TEMP") "/xcrun_db")))
(allow process-exec (literal "/bin/ps") (with no-sandbox))' "$@"
