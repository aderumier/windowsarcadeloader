#!/usr/bin/env python3
"""Moves the working game dumps to the Batocera machine as .squashfs images.

A working dump is a directory games/<system>/<dump>/ holding its <gameid>.windowsloader, whose
game's status in docs/IMPLEMENTATION.md (games table) is not BLOCKED, not tested or not playable
(the .windowsloader only says which executable to start).
Smallest dump first (each one moved frees room for the next), one at a time:
  1. mksquashfs <dump> -comp zstd -> games/<system>/<dump>.squashfs (dump contents at the root:
     Batocera's generator mounts the image and looks for the .windowsloader there);
  2. if the machine's rom directory has room for it, upload to
     /userdata/roms/<system>/<dump>.squashfs.part, md5 compared with the local image, then renamed
     to <dump>.squashfs;
  3. only then the local image and the dump directory are deleted.
Any error stops the run, before deleting anything of that dump (its local image is removed).

Skipped: games not working (status above) or missing from the games table, systems without a
Batocera ES system (games/misc...), images already on the machine
(--overwrite replaces them), dumps with absolute symlinks (broken inside an image).

Usage: tools/batocera/upload-dumps.py [--list] [--only TEXT]... [--skip TEXT]... [--keep] [--overwrite]
                                      [--host root@HOST] [--password PASS]
"""

import argparse
import hashlib
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
GAMES = ROOT / "games"
# local games/<dir> -> Batocera ES system (/userdata/roms/<system>)
SYSTEMS = {s: s for s in ["typex", "typex2", "nesicax", "nesicax2", "globalvr", "namcoes3", "rawthrills"]}
# Type X+ (Battle Gear 4) in Batocera's Type X system
SYSTEMS["typex+"] = "typex"
DOC = ROOT / "docs/IMPLEMENTATION.md"
# statuses of games that do not work
NOT_WORKING = ("blocked", "not tested", "not playable")
# free space kept besides the image, on the local disk and in the machine's rom directory
MARGIN = 1 << 30


def statuses() -> dict[str, str]:
    """Game id -> status of the games table of docs/IMPLEMENTATION.md."""
    out = {}
    for line in DOC.read_text().splitlines():
        if m := re.match(r"\| `([^`]+)` \| [^|]* \| [^|]* \| ([^|]*) \|", line):
            out[m.group(1)] = m.group(2).strip()
    return out


def dumps() -> list[tuple[int, Path]]:
    """(size in bytes, dump directory) of every dump holding a .windowsloader, smallest first."""
    out = []
    for system in sorted(GAMES.iterdir()):
        if not system.is_dir():
            continue
        for dump in sorted(system.iterdir()):
            if dump.is_dir() and any(dump.glob("*.windowsloader")):
                size = sum(f.lstat().st_size for f in dump.rglob("*") if f.is_file() or f.is_symlink())
                out.append((size, dump))
    return sorted(out)


def absolute_links(dump: Path) -> list[Path]:
    return [f for f in dump.rglob("*") if f.is_symlink() and os.readlink(f).startswith("/")]


def md5(path: Path) -> str:
    h = hashlib.md5()
    with path.open("rb") as f:
        while chunk := f.read(1 << 22):
            h.update(chunk)
    return h.hexdigest()


class Remote:
    """ssh/rsync to the machine through one master connection, password given by SSH_ASKPASS."""

    def __init__(self, host: str, password: str):
        self.host = host
        self.dir = Path(tempfile.mkdtemp(prefix="wal-upload-"))
        askpass = self.dir / "askpass"
        askpass.write_text(f"#!/bin/sh\necho {shlex.quote(password)}\n")
        askpass.chmod(0o700)
        self.env = dict(os.environ, SSH_ASKPASS=str(askpass), SSH_ASKPASS_REQUIRE="force")
        self.env.setdefault("DISPLAY", ":0")
        self.ssh = ["ssh", "-o", "StrictHostKeyChecking=accept-new", "-o", "ControlMaster=auto",
                    "-o", f"ControlPath={self.dir}/s", "-o", "ControlPersist=10m"]

    def run(self, command: str, check: bool = True) -> subprocess.CompletedProcess:
        return subprocess.run([*self.ssh, self.host, command], env=self.env, check=check,
                              capture_output=True, text=True)

    def upload(self, local: Path, remote: str):
        subprocess.run(["rsync", "--partial", "--info=progress2", "-e", shlex.join(self.ssh),
                        str(local), f"{self.host}:{remote}"], env=self.env, check=True)

    def close(self):
        subprocess.run([*self.ssh, "-O", "exit", self.host], env=self.env, capture_output=True)
        shutil.rmtree(self.dir, ignore_errors=True)


def gib(n: int) -> str:
    return f"{n / (1 << 30):.1f} GiB"


def list_games(todo: list, skipped: list, args) -> int:
    """--list: the games that would be converted, by system, marked when their image is already
    on the machine (skipped unless --overwrite), then the skipped dumps."""
    on_machine: set[str] | None = None
    remote = Remote(args.host, args.password)
    try:
        dirs = " ".join(f"/userdata/roms/{s}" for s in sorted({t[2] for t in todo}))
        out = remote.run(f"for d in {dirs}; do ls -1 \"$d\" 2>/dev/null | sed \"s|^|$d/|\"; done", check=False)
        if out.returncode == 0:
            on_machine = set(out.stdout.splitlines())
    finally:
        remote.close()
    if on_machine is None:
        print(f"({args.host} not reachable: images already there not marked)")
    count, total = 0, 0
    for system in sorted({t[2] for t in todo}):
        print(f"\n{system}:")
        for size, dump, _, game_status in sorted((t for t in todo if t[2] == system), key=lambda t: t[1].name.lower()):
            there = on_machine is not None and f"/userdata/roms/{system}/{dump.name}.squashfs" in on_machine
            mark = "  [on the machine: overwritten]" if there and args.overwrite else "  [on the machine: skipped]" if there else ""
            if not there or args.overwrite:
                count, total = count + 1, total + size
            print(f"  {gib(size):>9}  {dump.name}  ({game_status[:60]}){mark}")
    if skipped:
        print("\nnot converted:")
        for rel, reason in skipped:
            print(f"  {rel}: {reason}")
    print(f"\nto convert: {count} games, {gib(total)} (uncompressed), smallest first")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--list", "--dry-run", dest="list", action="store_true",
                   help="list the games that would be converted (and the skipped ones), change nothing")
    p.add_argument("--only", action="append", default=[], help="dumps whose path contains TEXT")
    p.add_argument("--skip", action="append", default=[], help="not the dumps whose path contains TEXT")
    p.add_argument("--keep", action="store_true", help="keep the local dump after the upload")
    p.add_argument("--overwrite", action="store_true", help="replace images already on the machine")
    p.add_argument("--host", default="root@batocera.fritz.box")
    p.add_argument("--password", default=os.environ.get("WAL_BATOCERA_PASSWORD", "linux"))
    args = p.parse_args()

    status = statuses()
    todo, skipped = [], []
    for size, dump in dumps():
        rel = dump.relative_to(GAMES)
        if args.only and not any(t in str(rel) for t in args.only):
            continue
        if any(t in str(rel) for t in args.skip):
            continue
        gameid = next(dump.glob("*.windowsloader")).stem
        game_status = status.get(gameid)
        system = SYSTEMS.get(dump.parent.name)
        if game_status is None:
            skipped.append((rel, f"{gameid} not in the games table of docs/IMPLEMENTATION.md"))
        elif game_status.lower().startswith(NOT_WORKING):
            skipped.append((rel, game_status))
        elif system is None:
            skipped.append((rel, f"no Batocera system for games/{dump.parent.name}"))
        elif links := absolute_links(dump):
            skipped.append((rel, f"absolute symlinks ({links[0].relative_to(dump)} ...)"))
        else:
            todo.append((size, dump, system, game_status))

    if args.list:
        return list_games(todo, skipped, args)
    for rel, reason in skipped:
        print(f"skip {rel}: {reason}")

    remote = Remote(args.host, args.password)
    image = None
    try:
        if remote:
            remote.run("true")  # reachable, password accepted: before building anything
        for size, dump, system, _ in todo:
            rel = dump.relative_to(GAMES)
            target = f"/userdata/roms/{system}/{dump.name}.squashfs"
            test = remote.run(f"test -e {shlex.quote(target)}", check=False)
            if test.returncode not in (0, 1):
                print(f"error: {args.host}: {test.stderr.strip()}")
                return 1
            if test.returncode == 0 and not args.overwrite:
                print(f"skip {rel}: {target} already on the machine")
                continue
            free = shutil.disk_usage(dump.parent).free
            if free < size + MARGIN:
                print(f"stop: {gib(free)} free, {rel} needs up to {gib(size + MARGIN)}")
                return 1
            image = dump.parent / f"{dump.name}.squashfs"
            print(f"== {rel} ({gib(size)})")
            image.unlink(missing_ok=True)
            subprocess.run(["mksquashfs", str(dump), str(image), "-comp", "zstd", "-quiet",
                            "-no-progress"], check=True)
            local_md5 = md5(image)
            print(f"   image {gib(image.stat().st_size)}, md5 {local_md5}, uploading to {target}")
            remote.run(f"mkdir -p /userdata/roms/{system}")
            # room in the machine's rom directory (df -P: POSIX columns, also busybox)
            avail = int(remote.run(f"df -Pk /userdata/roms/{system} | tail -1").stdout.split()[3]) * 1024
            if avail < image.stat().st_size + MARGIN:
                print(f"stop: {gib(avail)} free in /userdata/roms/{system}, "
                      f"{image.name} needs {gib(image.stat().st_size + MARGIN)}: local dump kept")
                return 1
            part = target + ".part"
            remote.upload(image, part)
            remote_md5 = remote.run(f"md5sum {shlex.quote(part)}").stdout.split()[0]
            if remote_md5 != local_md5:
                print(f"error: {part}: md5 {remote_md5}, local {local_md5}: local dump kept")
                return 1
            remote.run(f"chmod 644 {shlex.quote(part)} && mv -f {shlex.quote(part)} {shlex.quote(target)}")
            image.unlink()
            image = None
            if args.keep:
                print(f"   uploaded, local dump kept")
            else:
                shutil.rmtree(dump)
                print(f"   uploaded, local dump deleted")
    except subprocess.CalledProcessError as e:
        print(f"error: {shlex.join(map(str, e.cmd))} failed ({e.returncode}): {(e.stderr or '').strip()}")
        return 1
    finally:
        if image is not None:
            image.unlink(missing_ok=True)  # failed dump: its local image is not kept
        if remote:
            remote.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
