#!/usr/bin/env python3
"""Fixture trees and the checks the safety suite makes on them.

Every walk goes through directory descriptors, so trees deeper than PATH_MAX read like any other.
Nothing here uses the engine under test: content is hashed with sha256, attributes are read with
libc directly.
"""

import argparse
import ctypes
import hashlib
import json
import os
import random
import re
import stat
import sys
import time
import unicodedata

LIBC = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
LIBC.flistxattr.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int]
LIBC.flistxattr.restype = ctypes.c_ssize_t
LIBC.fgetxattr.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t,
                           ctypes.c_uint32, ctypes.c_int]
LIBC.fgetxattr.restype = ctypes.c_ssize_t
LIBC.acl_get_fd_np.argtypes = [ctypes.c_int, ctypes.c_int]
LIBC.acl_get_fd_np.restype = ctypes.c_void_p
LIBC.acl_to_text.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
LIBC.acl_to_text.restype = ctypes.c_void_p
LIBC.acl_free.argtypes = [ctypes.c_void_p]

O_SYMLINK = getattr(os, "O_SYMLINK", 0x00200000)
ACL_TYPE_EXTENDED = 0x00000100
LITTER = {".Trashes", ".fseventsd", ".Spotlight-V100", ".TemporaryItems",
          ".DocumentRevisions-V100", ".HFS+ Private Directory Data\r", ".journal",
          ".journal_info_block", "\u2400\u2400\u2400\u2400HFS+ Private Data"}
TEMP = ".finder-fs-"
EXACT = re.compile(r"\.finder-fs-(staging|replace)-[0-9]+-[0-9]+-[0-9]+")


def sha(fd):
    digest = hashlib.sha256()
    while True:
        chunk = os.read(fd, 1 << 20)
        if not chunk:
            return digest.hexdigest()
        digest.update(chunk)


def xattrs(fd):
    size = LIBC.flistxattr(fd, None, 0, 0)
    if size <= 0:
        return {}
    names = ctypes.create_string_buffer(size)
    size = LIBC.flistxattr(fd, names, size, 0)
    found = {}
    for name in names.raw[:max(size, 0)].split(b"\0"):
        if not name:
            continue
        length = LIBC.fgetxattr(fd, name, None, 0, 0, 0)
        value = ctypes.create_string_buffer(max(length, 1))
        length = LIBC.fgetxattr(fd, name, value, max(length, 0), 0, 0)
        found[name.decode("utf-8", "replace")] = hashlib.sha256(value.raw[:max(length, 0)]).hexdigest()
    return found


def acl(fd):
    handle = LIBC.acl_get_fd_np(fd, ACL_TYPE_EXTENDED)
    if not handle:
        return ""
    text = LIBC.acl_to_text(handle, None)
    shown = ctypes.string_at(text).decode("utf-8", "replace") if text else ""
    if text:
        LIBC.acl_free(text)
    LIBC.acl_free(handle)
    # The header line differs by volume; the entries are what a copy must keep.
    return "\n".join(line for line in shown.splitlines() if not line.startswith("!#acl"))


def describe(name, dirfd, st, meta):
    kind = stat.S_IFMT(st.st_mode)
    entry = {"type": {stat.S_IFREG: "file", stat.S_IFDIR: "dir", stat.S_IFLNK: "link"}.get(kind, "other")}
    if kind == stat.S_IFLNK:
        entry["link"] = os.readlink(name, dir_fd=dirfd)
    if kind == stat.S_IFREG:
        entry["size"] = st.st_size
    if meta:
        entry["mode"] = oct(stat.S_IMODE(st.st_mode))
        entry["flags"] = st.st_flags
        entry["birth"] = round(st.st_birthtime, 3)
        entry["birth_s"] = int(st.st_birthtime)
        if kind != stat.S_IFDIR:
            entry["mtime"] = round(st.st_mtime, 3)
            entry["mtime_s"] = int(st.st_mtime)
    flags = os.O_RDONLY | (O_SYMLINK if kind == stat.S_IFLNK else 0)
    try:
        fd = os.open(name, flags, dir_fd=dirfd)
    except OSError as error:
        entry["unreadable"] = error.strerror
        return entry
    try:
        if kind == stat.S_IFREG:
            entry["sha256"] = sha(fd)
        if meta:
            entry["xattrs"] = xattrs(fd)
            if kind != stat.S_IFLNK:
                entry["acl"] = acl(fd)
    finally:
        os.close(fd)
    return entry


def manifest(root, meta=False, ignore=()):
    """Every entry below `root`, keyed by its path relative to it."""
    found = {}
    for dirpath, dirnames, filenames, dirfd in os.fwalk(root, follow_symlinks=False):
        rel_dir = os.path.relpath(dirpath, root)
        at_root = rel_dir == "."
        for name in sorted(dirnames + filenames):
            if (at_root and name in LITTER) or name in ignore:
                if name in dirnames:
                    dirnames.remove(name)
                continue
            st = os.stat(name, dir_fd=dirfd, follow_symlinks=False)
            rel = name if at_root else os.path.join(rel_dir, name)
            found[rel] = describe(name, dirfd, st, meta)
    return found


class Deep:
    """Opens a path one part at a time, so a path longer than PATH_MAX still resolves."""

    def __init__(self, root, rel):
        self.parts = [part for part in rel.split(os.sep) if part]
        self.root = root

    def __enter__(self):
        self.fds = [os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)]
        try:
            for part in self.parts[:-1]:
                self.fds.append(os.open(part, os.O_RDONLY | os.O_DIRECTORY, dir_fd=self.fds[-1]))
        except OSError:
            self.__exit__()
            raise
        return self.fds[-1], self.parts[-1]

    def __exit__(self, *_):
        for fd in self.fds:
            os.close(fd)


def lstat_at(root, rel):
    try:
        with Deep(root, rel) as (dirfd, name):
            return os.stat(name, dir_fd=dirfd, follow_symlinks=False)
    except OSError:
        return None


def hash_at(root, rel):
    try:
        with Deep(root, rel) as (dirfd, name):
            fd = os.open(name, os.O_RDONLY | O_SYMLINK, dir_fd=dirfd)
            try:
                if not stat.S_ISREG(os.fstat(fd).st_mode):
                    return None
                return sha(fd)
            finally:
                os.close(fd)
    except OSError:
        return None


def files_of(listed):
    return {rel: entry for rel, entry in listed.items() if entry["type"] == "file"}


def temps(root, exact=False):
    found = []
    for dirpath, dirnames, filenames, _ in os.fwalk(root, follow_symlinks=False):
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == ".":
            dirnames[:] = [name for name in dirnames if name not in LITTER]
        for name in dirnames + filenames:
            if (EXACT.fullmatch(name) if exact else name.startswith(TEMP)):
                found.append(os.path.normpath(os.path.join(rel_dir, name)))
    return found


def compare(one, other, fields):
    """Differences between two manifests, in the fields asked for."""
    problems = []
    for rel in sorted(set(one) | set(other)):
        if rel not in other:
            problems.append(f"missing {rel!r}")
            continue
        if rel not in one:
            problems.append(f"extra {rel!r}")
            continue
        for field in fields:
            a, b = one[rel].get(field), other[rel].get(field)
            if a != b:
                problems.append(f"{rel!r} {field}: {a!r} != {b!r}")
    return problems


def cmd_manifest(args):
    json.dump(manifest(args.dir, args.meta, set(args.ignore or ())), sys.stdout, indent=1, sort_keys=True)
    print()


def keyed(listed, skip_temps, nfd):
    """HFS+ stores names decomposed; a temp is litter a killed run may leave."""
    out = {}
    for rel, entry in listed.items():
        if skip_temps and any(part.startswith(TEMP) for part in rel.split(os.sep)):
            continue
        out[unicodedata.normalize("NFD", rel) if nfd else rel] = entry
    return out


def cmd_compare(args):
    with open(args.one) as one, open(args.other) as other:
        one, other = (keyed(json.load(side), args.skip_temps, args.nfd) for side in (one, other))
        problems = compare(one, other, args.fields.split(","))
    for problem in problems[:40]:
        print(problem)
    if len(problems) > 40:
        print(f"... and {len(problems) - 40} more")
    return 1 if problems else 0


def cmd_split(args):
    """After a kill: each file whole at its source or whole at its destination, nothing partial."""
    with open(args.manifest) as handle:
        wanted = files_of(json.load(handle))
    lost, partial, both, moved = [], [], 0, 0
    for rel, entry in wanted.items():
        here = hash_at(args.source, rel)
        there = hash_at(args.dest, rel)
        if there is not None and there != entry["sha256"]:
            partial.append(rel)
        if entry["sha256"] not in (here, there):
            lost.append(rel)
        both += here == entry["sha256"] and there == entry["sha256"]
        moved += here is None and there == entry["sha256"]
    report = {"files": len(wanted), "lost": lost, "partial_under_real_name": partial,
              "at_both": both, "only_at_dest": moved, "temps": temps(args.dest)}
    json.dump(report, sys.stdout, sort_keys=True)
    print()
    return 1 if lost or partial else 0


def cmd_watch(args):
    """Polls a move: a source may vanish only once its destination holds all of it."""
    with open(args.manifest) as handle:
        wanted = files_of(json.load(handle))
    pending = dict(wanted)
    gone, early, polls = {}, [], 0
    start = time.monotonic()
    while not os.path.exists(args.stop) and time.monotonic() - start < args.limit:
        polls += 1
        for rel in list(pending):
            if lstat_at(args.source, rel) is not None:
                continue
            landed = lstat_at(args.dest, rel)
            if landed is None or landed.st_size != wanted[rel]["size"]:
                early.append({"file": rel, "dest_size": landed.st_size if landed else None})
            gone[rel] = time.monotonic() - start
            del pending[rel]
        time.sleep(0.0005)
    wrong = [rel for rel in gone if hash_at(args.dest, rel) != wanted[rel]["sha256"]]
    report = {"files": len(wanted), "sources_removed": len(gone), "polls": polls,
              "removed_before_copy_whole": early, "dest_differs": wrong}
    with open(args.out, "w") as handle:
        json.dump(report, handle, sort_keys=True)
    return 1 if early or wrong else 0


def cmd_temps(args):
    for rel in temps(args.dir, args.exact):
        print(rel)


def cmd_count(args):
    listed = manifest(args.dir)
    files = sum(entry["type"] == "file" for entry in listed.values())
    print(json.dumps({"entries": len(listed), "files": files, "dirs": len(listed) - files}))


def cmd_tree(args):
    """Deterministic random content: a seed always makes the same bytes."""
    rng = random.Random(args.seed)
    os.makedirs(args.dir, exist_ok=True)
    for index in range(args.count):
        folder = args.dir
        for level in range(rng.randrange(args.depth + 1)):
            folder = os.path.join(folder, f"d{level}-{rng.randrange(args.fanout)}")
        os.makedirs(folder, exist_ok=True)
        size = rng.randint(args.min, args.max)
        with open(os.path.join(folder, f"f{index:06d}.bin"), "wb") as out:
            out.write(rng.randbytes(size) if hasattr(rng, "randbytes") else os.urandom(size))
    for spec in args.big or ():
        name, size = spec.split(":")
        with open(os.path.join(args.dir, name), "wb") as out:
            left = int(size)
            while left:
                chunk = min(left, 1 << 20)
                out.write(rng.randbytes(chunk) if hasattr(rng, "randbytes") else os.urandom(chunk))
                left -= chunk


def cmd_deep(args):
    """A chain of folders whose full path runs past PATH_MAX, with a file at each of a few levels."""
    fd = os.open(args.dir, os.O_RDONLY | os.O_DIRECTORY)
    length = len(os.path.abspath(args.dir))
    level = 0
    try:
        while length < args.length:
            name = f"level-{level:03d}-" + "x" * 40
            os.mkdir(name, dir_fd=fd)
            inner = os.open(name, os.O_RDONLY | os.O_DIRECTORY, dir_fd=fd)
            os.close(fd)
            fd = inner
            length += len(name) + 1
            if level % 8 == 0:
                out = os.open(f"file-{level}.txt", os.O_WRONLY | os.O_CREAT, 0o644, dir_fd=fd)
                os.write(out, os.urandom(args.file_size) if args.file_size else (f"level {level}\n" * 2000).encode())
                os.close(out)
            level += 1
    finally:
        os.close(fd)
    print(json.dumps({"levels": level, "length": length}))


def cmd_sparse(args):
    with open(args.path, "wb") as out:
        out.truncate(args.size)


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("manifest")
    sub.add_argument("dir")
    sub.add_argument("--meta", action="store_true")
    sub.add_argument("--ignore", action="append")
    sub.set_defaults(run=cmd_manifest)
    sub = commands.add_parser("compare")
    sub.add_argument("one")
    sub.add_argument("other")
    sub.add_argument("--fields", default="type,size,sha256,link")
    sub.add_argument("--skip-temps", action="store_true")
    sub.add_argument("--nfd", action="store_true")
    sub.set_defaults(run=cmd_compare)
    sub = commands.add_parser("split")
    sub.add_argument("source")
    sub.add_argument("dest")
    sub.add_argument("manifest")
    sub.set_defaults(run=cmd_split)
    sub = commands.add_parser("watch")
    sub.add_argument("source")
    sub.add_argument("dest")
    sub.add_argument("manifest")
    sub.add_argument("stop")
    sub.add_argument("out")
    sub.add_argument("--limit", type=float, default=600)
    sub.set_defaults(run=cmd_watch)
    sub = commands.add_parser("temps")
    sub.add_argument("dir")
    sub.add_argument("--exact", action="store_true")
    sub.set_defaults(run=cmd_temps)
    sub = commands.add_parser("count")
    sub.add_argument("dir")
    sub.set_defaults(run=cmd_count)
    sub = commands.add_parser("tree")
    sub.add_argument("dir")
    sub.add_argument("--count", type=int, default=100)
    sub.add_argument("--min", type=int, default=0)
    sub.add_argument("--max", type=int, default=65536)
    sub.add_argument("--depth", type=int, default=3)
    sub.add_argument("--fanout", type=int, default=4)
    sub.add_argument("--seed", type=int, default=1)
    sub.add_argument("--big", action="append", help="name:bytes")
    sub.set_defaults(run=cmd_tree)
    sub = commands.add_parser("deep")
    sub.add_argument("dir")
    sub.add_argument("--length", type=int, default=1400)
    sub.add_argument("--file-size", type=int, default=0)
    sub.set_defaults(run=cmd_deep)
    sub = commands.add_parser("sparse")
    sub.add_argument("path")
    sub.add_argument("size", type=int)
    sub.set_defaults(run=cmd_sparse)
    args = parser.parse_args()
    sys.exit(args.run(args) or 0)


if __name__ == "__main__":
    main()
