#!/usr/bin/env python3
"""The disk space folders and files take, counting each block once.

`du` counts a block once per file that holds it unless the files are hard
links of one inode. APFS clones are separate inodes sharing blocks: cargo
uplifts deps/libbae_bridge.a to debug/libbae_bridge.a as a clone, and `du`
read bae's target-macos as 9.3 GB where it took 7.3 GB. A file of at least
CLONE_MIN is measured by its physical extents (fcntl F_LOG2PHYS_EXT), each
counted once; a clone of a smaller file would shift a total by too little to
look for.

    disk_usage.py <path>    prints the KiB <path> takes, 0 when it is missing
"""

import fcntl
import os
import stat
import struct
import sys

F_LOG2PHYS_EXT = 65
# struct log2phys from <sys/fcntl.h>, packed: l2p_flags, l2p_contigbytes,
# l2p_devoffset. In: the bytes to look up and the file offset; out: the
# contiguous bytes from there and their offset on the device.
LOG2PHYS = struct.Struct("=Iqq")
BLOCK = 4096
CLONE_MIN = 1 << 20


def extents(path, size):
    """(<device offset>, <bytes>) of each physical extent of <path>."""
    found = []
    fd = os.open(path, os.O_RDONLY)
    try:
        offset = 0
        while offset < size:
            _, contiguous, device = LOG2PHYS.unpack(
                fcntl.fcntl(fd, F_LOG2PHYS_EXT, LOG2PHYS.pack(0, size - offset, offset)))
            if contiguous <= 0:
                break
            found.append((device, contiguous))
            offset += contiguous
    finally:
        os.close(fd)
    return found


class Usage:
    """Bytes on disk, counting each hard-linked inode and each cloned extent
    once across everything one Usage measures."""

    def __init__(self):
        self.inodes = set()
        self.extents = set()

    def entry(self, path, st):
        if st.st_nlink > 1 and not stat.S_ISDIR(st.st_mode):
            key = (st.st_dev, st.st_ino)
            if key in self.inodes:
                return 0
            self.inodes.add(key)
        if stat.S_ISREG(st.st_mode) and st.st_blocks * 512 >= CLONE_MIN:
            try:
                found = extents(path, st.st_size)
            except OSError:
                # Compressed or holed files have no plain extent map.
                found = None
            if found:
                total = 0
                for device, size in found:
                    key = (st.st_dev, device)
                    if key not in self.extents:
                        self.extents.add(key)
                        total += -(-size // BLOCK) * BLOCK
                return total
        return st.st_blocks * 512

    def of(self, path):
        """Bytes <path> takes, a file or a whole folder; 0 when missing."""
        try:
            st = os.lstat(path)
        except FileNotFoundError:
            return 0
        total = self.entry(path, st)
        if stat.S_ISDIR(st.st_mode):
            for root, dirs, files in os.walk(path):
                for name in dirs + files:
                    child = os.path.join(root, name)
                    try:
                        total += self.entry(child, os.lstat(child))
                    except (FileNotFoundError, PermissionError):
                        pass
        return total


def main(argv):
    if len(argv) != 1:
        print("usage: disk_usage.py <path>", file=sys.stderr)
        return 2
    print(Usage().of(argv[0]) // 1024)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
