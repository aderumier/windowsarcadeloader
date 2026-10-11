#!/usr/bin/env python3
"""Binary hotfixes for Wine runners, until the fixes ship in a runner release.

Usage: tools/runner-hotfixes.py [runner dir...]   (default: every runner in wine-runners/)

Each fix matches an exact byte sequence (unique in the file) and is skipped when the runner
does not contain it (other builds, already fixed). The original file is kept as
<runner>/hotfix-backups/<dll dir>/<file>.orig: never next to the runner's DLLs, which the
prefix update copies into system32/syswow64 and registers (a `qasf.dll.pre-*` backup there
became the registered DirectShow DMO wrapper and ASF reader).

* winedmo-wow64-demuxer-destroy: 32-bit games, `wow64_demuxer_destroy` passes a
  `demuxer_create_params` to `demuxer_destroy`, which reads the demuxer handle from the
  uninitialized `url` field: leaked demuxers, faults, and a glibc "free(): invalid size" abort
  when a DirectShow movie is stopped (Battle Fantasia: coin during the attract movie).
  Upstream Wine 2110c64d89cc; compiled fix: store the handle at offset 0 of the params.
* mountmgr-serial-probe (x86_64 and i386): mountmgr checks each /dev/ttyS*, ttyUSB*, ttyACM*
  by opening it without O_NOCTTY in winedevice.exe, a session leader without a terminal
  (setsid): the first one becomes its controlling tty, hung up when the game exits. A Sinden
  lightgun's ttyACM (its software's link to the gun) then stays dead: one of two guns no
  longer moved nor clicked after each game. The probe is skipped (the `params->serial` test
  jumps over it, as the 002-fix-mountmgr-serial-noctty patch of the loader's Wine builds).
  The Batocera runner ships mountmgr.so rebuilt from the GE sources without the probe (not
  applicable there); this byte patch is for a fresh GE-Proton11-7 download.
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
    (
        "mountmgr-serial-probe",
        "lib/wine/x86_64-unix/mountmgr.so",
        # mov 0x10(%rdi),%eax; test %eax,%eax; je +0x2d (probe); mov 0x8(%rdi),%rdi; xor %esi,%esi; xor %eax,%eax
        bytes.fromhex("8b471085c0742d488b7f0831f631c0"),
        # ...; jmp +0x2d
        bytes.fromhex("8b471085c0eb2d488b7f0831f631c0"),
    ),
    (
        "mountmgr-serial-probe",
        "lib/wine/i386-unix/mountmgr.so",
        # mov 0x8(%edi),%ecx; test %ecx,%ecx; je +0x3c (probe); sub $8,%esp; push $0; push 0x4(%edi)
        bytes.fromhex("8b4f0885c9743c83ec086a00ff7704"),
        # ...; jmp +0x3c
        bytes.fromhex("8b4f0885c9eb3c83ec086a00ff7704"),
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
        backup = runner / "hotfix-backups" / path.parent.name / (path.name + ".orig")
        if not backup.exists():
            backup.parent.mkdir(parents=True, exist_ok=True)
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
