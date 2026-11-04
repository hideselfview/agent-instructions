#!/usr/bin/env python3
"""When each folder was last written to: the newest modification time, in
seconds since 1970, of the folder, its entries and their entries (what
`find -maxdepth 2` reaches).

    newest_write.py <path>...    prints one time per path, in order; an empty
                                 line when nothing under it can be read

Eight folders are read at a time; see disk_usage.py.
"""

import multiprocessing
import os
import sys

WORKERS = 8


def newest(path):
    times = []

    def note(p):
        try:
            times.append(int(os.stat(p).st_mtime))
        except OSError:
            pass  # a broken link or a file gone since it was listed

    note(path)
    try:
        for first in os.scandir(path):
            note(first.path)
            if first.is_dir(follow_symlinks=False):
                try:
                    for second in os.scandir(first.path):
                        note(second.path)
                except OSError:
                    pass
    except OSError:
        pass
    return max(times) if times else ""


def main(argv):
    if not argv:
        print("usage: newest_write.py <path>...", file=sys.stderr)
        return 2
    with multiprocessing.get_context("fork").Pool(min(WORKERS, len(argv))) as pool:
        for when in pool.map(newest, argv, chunksize=1):
            print(when)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
