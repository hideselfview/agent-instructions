#!/usr/bin/env python3
"""Build output in the temporary folders, which belongs on the build volume.

A build pointed at a temporary folder (CARGO_TARGET_DIR=/tmp/..., xcodebuild
-derivedDataPath /tmp/..., a probe crate or package built where it was made)
writes 5-14 GB there that nothing deletes: before `builds` covered them, such
folders piled up past 100 GB in /private/tmp. `builds prune` and `setup`
delete each one nothing wrote to lately; this lists them.

    temp_builds.py <folder>...   prints each build output folder found under
                                 the folders, outermost only, one per line

Three kinds are recognized by what their tool writes at their top:

    a cargo target folder         .rustc_info.json
    an Xcode DerivedData folder   info.plist beside Logs, and Build or
                                  ModuleCache.noindex
    a SwiftPM folder              workspace-state.json beside checkouts,
                                  repositories or artifacts

At the top of each folder given, entries named claude-*, com.apple.* and
tmux-* are skipped whole: Claude Code sessions' scratch folders (which
`builds` prunes by their own rule), launchd's per-session socket folders, and
the tmux server's socket folder, whose deletion strands a running server.
Only folders the caller owns are listed, and symlinks are never followed.
"""

import os
import sys

# Deep enough for /private/tmp/<probe>/<crate>/target and
# $TMPDIR/<tool>-<id>/<package>/.build; a build folder's own contents are
# never walked.
MAX_DEPTH = 4
SKIPPED_PREFIXES = ("claude-", "com.apple.", "tmux-")


def kind(path, names):
    if ".rustc_info.json" in names:
        return "cargo"
    if "info.plist" in names and "Logs" in names and (
        "Build" in names or "ModuleCache.noindex" in names
    ):
        return "derived-data"
    if "workspace-state.json" in names and (
        "checkouts" in names or "repositories" in names or "artifacts" in names
    ):
        return "swiftpm"
    return None


def walk(top, uid):
    found = []
    for root, dirs, files in os.walk(top):
        depth = 0 if root == top else root[len(top):].count(os.sep)
        if root == top:
            dirs[:] = [d for d in dirs if not d.startswith(SKIPPED_PREFIXES)]
        else:
            try:
                if os.lstat(root).st_uid != uid:
                    dirs[:] = []
                    continue
            except OSError:
                dirs[:] = []
                continue
            if kind(root, set(dirs) | set(files)):
                found.append(root)
                dirs[:] = []
                continue
        if depth >= MAX_DEPTH:
            dirs[:] = []
    return found


def main(argv):
    if not argv:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    uid = os.getuid()
    for top in argv:
        top = os.path.realpath(top)
        if os.path.isdir(top):
            for path in walk(top, uid):
                print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
