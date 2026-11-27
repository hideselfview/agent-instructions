#!/bin/sh
# Runs a test where macOS refuses each write outside /dev and the run's
# temporary folder (.config/nextest.toml), with these exceptions. A test that
# replays FSEvents history makes its folder in ~/Library/Caches/builds-tests
# (src/testing/history_dir.rs). The shell makes the file of each here-document
# in /private/var/tmp, whatever TMPDIR says, once it finds that folder
# writable. Each of the Mac's developer tools, git among them, keeps its
# xcrun_db cache of tool paths in the user's temporary folder. ps is setuid,
# which a sandboxed process may not run, and writes nothing. Chrome keeps its
# socket folder in the user's temporary folder, whatever TMPDIR says, unless
# MAC_CHROMIUM_TMPDIR names another.
export MAC_CHROMIUM_TMPDIR="${TMPDIR:?run the tests with builds cargo, which gives the run a temporary folder}"
run_temp=$(cd "$TMPDIR" && pwd -P) || exit 1
user_temp=$(cd "$(getconf DARWIN_USER_TEMP_DIR)" && pwd -P) || exit 1
exec /usr/bin/sandbox-exec -D "RUN_TEMP=$run_temp" -D "USER_TEMP=$user_temp" \
    -D "HISTORY=$HOME/Library/Caches/builds-tests" -p '(version 1)
(allow default)
(deny file-write*)
(allow file-write* (subpath "/dev") (subpath (param "RUN_TEMP")) (subpath (param "HISTORY")))
(allow file-write* (literal "/private/var/tmp") (regex #"^/private/var/tmp/sh-thd-"))
(allow file-write* (regex (string-append "^" (param "USER_TEMP") "/xcrun_db")))
(allow process-exec (literal "/bin/ps") (with no-sandbox))' "$@"
