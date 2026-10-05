#!/usr/bin/env python3
"""Binary hotfixes for Wine runners, until the fixes ship in a runner release.

Usage: tools/runner-hotfixes.py [runner dir...]   (default: every runner in wine-runners/)

Each fix matches an exact byte sequence (unique in the file) and is skipped when the runner
does not contain it (other builds, already fixed). The original file is kept as <file>.orig.

* winedmo-wow64-demuxer-destroy: 32-bit games, `wow64_demuxer_destroy` passes a
  `demuxer_create_params` to `demuxer_destroy`, which reads the demuxer handle from the
  uninitialized `url` field: leaked demuxers, faults, and a glibc "free(): invalid size" abort
  when a DirectShow movie is stopped (Battle Fantasia: coin during the attract movie).
  Upstream Wine 2110c64d89cc; compiled fix: store the handle at offset 0 of the params.
"""

import pathlib
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

FIXES = [
    (
        "winedmo-wow64-demuxer-destroy",
        "lib/wine/x86_64-unix/winedmo.so",
        # mov (%rdi),%rax; lea -0x140(%rbp),%rdi; mov %rax,-0x130(%rbp)
        bytes.fromhex("488b07488dbdc0feffff488985d0feffff"),
        # ...; mov %rax,-0x140(%rbp)
        bytes.fromhex("488b07488dbdc0feffff488985c0feffff"),
    ),
]


def apply(runner: pathlib.Path) -> None:
    for name, rel, old, new in FIXES:
        path = runner / rel
        if not path.is_file():
            continue
        data = path.read_bytes()
        if data.count(old) != 1:
            state = "applied" if data.count(new) == 1 else "not applicable"
            print(f"{runner.name}: {name}: {state}")
            continue
        backup = path.with_name(path.name + ".orig")
        if not backup.exists():
            shutil.copy2(path, backup)
        tmp = path.with_name(path.name + ".tmp")
        tmp.write_bytes(data.replace(old, new))
        shutil.copymode(path, tmp)
        tmp.replace(path)
        print(f"{runner.name}: {name}: patched (backup {backup.name})")


def main() -> None:
    runners = [pathlib.Path(a) for a in sys.argv[1:]] or sorted(p for p in (ROOT / "wine-runners").iterdir() if p.is_dir())
    for runner in runners:
        apply(runner)


if __name__ == "__main__":
    main()
