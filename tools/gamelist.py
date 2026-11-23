#!/usr/bin/env python3
"""Game list of README.md: the status table of docs/IMPLEMENTATION.md (one row per game id)
with each game's latest regression test result (tools/regression/results/*/results.json), its
network play (the cabinet link: the profile sets WAL_LINK) and its network outputs (MAME's,
WAL_OUTPUTS: the games' own in OUTPUTS, the boards every game of a system goes through in
SYSTEM_OUTPUTS, a RingEdge drive board from WAL_RINGEDGE_FFB).

Only plain runs of this tree count: runs marked `experiment` (regression.py --profile, or by
hand for a run with an uncommitted change), with an extra profile layer or another tree (--root)
are skipped; a clean worktree of this repository (the deployed commit) counts as this tree. The
games of a Batocera roms run (--roms) are named by their image's id
(tools/regression/roms-index.json). Rewrites README.md between the GAMELIST markers, and the
games' count in its "## Games" title.

Usage: tools/gamelist.py
"""

import json
import re
import subprocess
from functools import cache
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "docs/IMPLEMENTATION.md"
README = ROOT / "README.md"
RESULTS = ROOT / "tools/regression/results"
ROMS_INDEX = ROOT / "tools/regression/roms-index.json"
CHECKS = ["started", "running", "window", "picture", "audio"]
PROFILES = ROOT / "systemprofiles"

# The network outputs of the games that have their own (docs/IMPLEMENTATION.md §6.9); "not
# seen": wired, not seen working in a test
GUN = "lamps, recoil, damage, life, ammo"
OUTPUTS = {
    "lethal-enforcers-3": GUN,
    "lethal-enforcers-3-2p": GUN,
    "haunted-museum": GUN.replace(", ammo", ""),
    "haunted-museum-2": "lamps, recoil",
    "gaia-attack-4": GUN.replace(", ammo", ""),
    "elevator-action-death-parade": GUN,
    "music-gungun-2": "lamps",
    "castlevania-the-arcade": "lamps, damage, life, ammo",
    "coopers-9": GUN,
    "wartran-troopers": GUN + " (not seen)",
    "silent-hill-the-arcade": "lamps, recoil",
    "akuma-mortis-immortal": "lamps, recoil, damage, life",
    "bug-busters": "lamps, air blowers, recoil, damage, life, ammo",
    "the-house-of-the-dead-3": "start lamps, recoil, damage, life, ammo",
    "robin-hood": "recoil",
    "plants-vs-zombies-the-last-stand": "lamps, recoil, ticket motor",
    "mad-bullets": "recoil, damage, life, ammo",
    "house-of-the-dead-overkill": "recoil, damage, life, ammo (not seen)",
    "night-hunter-after-dark-chapter-ii": "recoil, gun lights, air jets, wind",
    "night-hunter-after-dark-chapter-ii-3-0-2": "recoil, gun lights, air jets, wind",
    "men-in-black": "recoil",
    "dream-raiders": "lamps, gun motors, blower, damage, life",
    "lets-go-island-ringwide": "lamps, recoil, gun motors, damage, life",
    "medal-no-gunman": "recoil, ammo",
    "sega-golden-gun": "lamps, recoil, damage, life, ammo",
    "transformers-human-alliance": "lamps, recoil, gun motors, damage, life",
    "transformers-shadows-rising": "lamps, recoil, damage",
    "top-gun-arcade": "lamps, recoil, prize doors, tickets",
    "quick-shot-arcade": "lamps, recoil, ammo, prize doors, tickets",
    "dcop": "lamps, recoil, gun light, life, ammo",
    "mech-dinosaur": "lamps, ball / water guns, seat shaker",
    "wisdom-zombies": "lamps, water guns, gun motors, seat shaker (not seen)",
    "top-gun-2": "lamps (not seen)",
    "mech-tornado": "lamps, water guns, motors, shaker, pump",
    "fire-hero": "lamps, water guns, gun motors, chair shaker, pump",
    "sin": "recoil",
    "storm-racer-g": "lamps, PWM lights",
    "storm-rider": "lamps, PWM lights",
    "crazy-ride": "lights",
    "gti-club-supermini-festa": "lamps, brightness (not seen)",
    "wasteland-racers-2071": "lamps",
    "mario-kart-arcade-gp-dx": "lamps, LEDs (not seen)",
    "bishi-bashi-channel": "start lamps",
    "gashaaaan": "lamps (not seen)",
    "otomedius": "lamps (not seen)",
    "winning-eleven-2008": "lamps (not seen)",
}
# The outputs of the I/O boards every game of a system goes through
SYSTEM_OUTPUTS = {
    "typex": "lamps (JVS)",
    "ringedge": "lamps (JVS)",
    "ringedge2": "lamps (JVS)",
    "seganu": "lamps (JVS)",
    "namcoes3": "lamps (JVS)",
    "alls": "lamps (IO4)",
}
BEGIN, END = "<!-- GAMELIST BEGIN (tools/gamelist.py) -->", "<!-- GAMELIST END -->"


def doc_games() -> list[dict]:
    """Rows of the games status table: id, game, system, result."""
    games, in_table = [], False
    for line in DOC.read_text().splitlines():
        if line.startswith("| Game id |"):
            in_table = True
            continue
        if in_table and line.startswith("|---"):
            continue
        if in_table and not line.startswith("| `"):
            if games:
                break
            continue
        if in_table:
            cells = [c.strip() for c in line.strip().strip("|").split(" | ")]
            games.append({"id": cells[0].strip("`"), "game": cells[1], "system": cells[2], "result": cells[3]})
    return games


@cache
def this_tree(root: Path) -> bool:
    """This checkout, or a worktree of its repository without changes."""
    if root == ROOT:
        return True
    git = lambda *args: subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True)
    common = git("rev-parse", "--path-format=absolute", "--git-common-dir")
    return (common.returncode == 0 and Path(common.stdout.strip()) == ROOT / ".git"
            and git("status", "--porcelain").stdout == "")


def plain_run(run: Path, name: str) -> bool:
    """The game's launcher.log shows the default layers only: this tree's launcher.yaml, the
    system profile and the run's regression.yaml."""
    log = run / name / "launcher.log"
    if not log.is_file():
        return False
    m = re.search(r"^profile layers: (\[.*\])$", log.read_text(errors="replace"), re.M)
    if not m:
        return False
    layers = json.loads(m.group(1))
    profiles = [Path(l) for l in layers if not l.endswith("regression.yaml")]
    if not profiles or profiles[0].name != "launcher.yaml" or not this_tree(profiles[0].parent):
        return False
    tree = str(profiles[0].parent) + "/"
    return all(str(l).startswith(tree) for l in profiles) and not any(
        "/tmp/" in l for l in layers) and len(profiles) <= 3


def latest_results() -> dict[str, tuple[str, dict]]:
    """game id -> (run date, result) of its latest plain run."""
    images = {}
    if ROMS_INDEX.is_file():
        for key, gid in json.loads(ROMS_INDEX.read_text()).items():
            if gid:
                images[key.split("|")[0]] = gid
    latest = {}
    for run in sorted(p for p in RESULTS.glob("*") if (p / "results.json").is_file() and not (p / "experiment").exists()):
        for rel, res in json.loads((run / "results.json").read_text()).items():
            name = Path(rel).stem
            gid = next((g for path, g in images.items() if path.endswith("/" + rel)), name) \
                if rel.endswith(".squashfs") else name
            if "skipped" in res or not plain_run(run, name):
                continue
            latest[gid] = (run.name[:8], res)
    return latest


def profile(gid: str, system: str) -> str:
    """The game's system profile, its text ("" without one)."""
    path = next(PROFILES.glob(f"*/{gid}.yaml"), None)
    return path.read_text() if path else ""


def network_cell(text: str) -> str:
    return "cabinet link" if re.search(r"^\s+WAL_LINK:", text, re.M) else ""


def outputs_cell(gid: str, system: str, text: str) -> str:
    parts = []
    own = OUTPUTS.get(gid)
    generic = SYSTEM_OUTPUTS.get(system)
    if generic and not (own and own.startswith("lamps")):
        parts.append(generic)
    if own:
        parts.append(own)
    if re.search(r"^\s+WAL_RINGEDGE_FFB:", text, re.M):
        parts.append("drive board")
    return ", ".join(parts)


def result_cell(res: dict) -> str:
    failed = [c for c in CHECKS if res.get(c) is False]
    if not failed:
        return "pass"
    return "fail: " + ", ".join(failed)


def table() -> str:
    latest = latest_results()
    lines = ["| Game id | Game | System | Status | Network play | Outputs | Regression test | Run |",
             "|---|---|---|---|---|---|---|---|"]
    for g in doc_games():
        date, res = latest.get(g["id"], (None, None))
        test = result_cell(res) if res else "not in the test"
        run = f"{date[:4]}-{date[4:6]}-{date[6:]}" if date else ""
        text = profile(g["id"], g["system"])
        net, out = network_cell(text), outputs_cell(g["id"], g["system"], text)
        lines.append(f"| `{g['id']}` | {g['game']} | {g['system']} | {g['result']} | {net} | {out} | {test} | {run} |")
    return "\n".join(lines)


def main():
    text = README.read_text()
    start, end = text.index(BEGIN) + len(BEGIN), text.index(END)
    text = text[:start] + "\n" + table() + "\n" + text[end:]
    # the games' count in the section's title
    count = len(doc_games())
    text = re.sub(r"^## Games( \(\d+\))?$", f"## Games ({count})", text, count=1, flags=re.M)
    README.write_text(text)
    print(f"{README}: {count} games")


if __name__ == "__main__":
    main()
