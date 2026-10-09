#!/usr/bin/env python3
"""Moves the working game dumps to the Batocera machine as .squashfs images.

A working dump is a directory games/<system>/<dump>/ holding its <gameid>.windowsloader.
Smallest dump first (each one moved frees room for the next), one at a time:
  1. mksquashfs <dump> -comp zstd -> games/<system>/<dump>.squashfs (dump contents at the root:
     Batocera's generator mounts the image and looks for the .windowsloader there);
  2. upload to /userdata/roms/<system>/<dump>.squashfs.part, md5 compared with the local image,
     then renamed to <dump>.squashfs;
  3. only then the local image and the dump directory are deleted.
Any error stops the run, before deleting anything of that dump.

Skipped: systems without a Batocera ES system (games/misc...), images already on the machine
(--overwrite replaces them), dumps with absolute symlinks (broken inside an image).

Usage: tools/batocera/upload-dumps.py [--dry-run] [--only TEXT]... [--keep] [--overwrite]
                                      [--host root@HOST] [--password PASS]
"""

import argparse
import hashlib
import os
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
# free space kept on the local disk besides the image being built
MARGIN = 1 << 30


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


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--dry-run", action="store_true", help="list what would be moved, change nothing")
    p.add_argument("--only", action="append", default=[], help="dumps whose path contains TEXT")
    p.add_argument("--keep", action="store_true", help="keep the local dump after the upload")
    p.add_argument("--overwrite", action="store_true", help="replace images already on the machine")
    p.add_argument("--host", default="root@batocera.fritz.box")
    p.add_argument("--password", default=os.environ.get("WAL_BATOCERA_PASSWORD", "linux"))
    args = p.parse_args()

    todo = []
    for size, dump in dumps():
        rel = dump.relative_to(GAMES)
        if args.only and not any(t in str(rel) for t in args.only):
            continue
        system = SYSTEMS.get(dump.parent.name)
        if system is None:
            print(f"skip {rel}: no Batocera system for games/{dump.parent.name}")
            continue
        if links := absolute_links(dump):
            print(f"skip {rel}: absolute symlinks ({links[0].relative_to(dump)} ...)")
            continue
        todo.append((size, dump, system))

    remote = None if args.dry_run else Remote(args.host, args.password)
    try:
        for size, dump, system in todo:
            rel = dump.relative_to(GAMES)
            target = f"/userdata/roms/{system}/{dump.name}.squashfs"
            if remote is None:
                print(f"{gib(size):>9}  {rel} -> {target}")
                continue
            exists = remote.run(f"test -e {shlex.quote(target)}", check=False).returncode == 0
            if exists and not args.overwrite:
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
            part = target + ".part"
            remote.upload(image, part)
            remote_md5 = remote.run(f"md5sum {shlex.quote(part)}").stdout.split()[0]
            if remote_md5 != local_md5:
                print(f"error: {part}: md5 {remote_md5}, local {local_md5}: local dump kept")
                return 1
            remote.run(f"chmod 644 {shlex.quote(part)} && mv -f {shlex.quote(part)} {shlex.quote(target)}")
            image.unlink()
            if args.keep:
                print(f"   uploaded, local dump kept")
            else:
                shutil.rmtree(dump)
                print(f"   uploaded, local dump deleted")
    except subprocess.CalledProcessError as e:
        print(f"error: {shlex.join(map(str, e.cmd))} failed ({e.returncode}): {(e.stderr or '').strip()}")
        return 1
    finally:
        if remote:
            remote.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
