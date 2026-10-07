#!/usr/bin/env python3
"""Game list of README.md: the status table of docs/IMPLEMENTATION.md (one row per game id)
with each game's latest regression test result (tools/regression/results/*/results.json).

Only plain runs of this tree count: runs marked `experiment` (regression.py --profile, or by
hand for a run with an uncommitted change), with an extra profile layer or another tree (--root)
are skipped. Rewrites README.md between the GAMELIST markers.

Usage: tools/gamelist.py
"""

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "docs/IMPLEMENTATION.md"
README = ROOT / "README.md"
RESULTS = ROOT / "tools/regression/results"
CHECKS = ["started", "running", "window", "picture", "audio"]
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
    return all(str(Path(l)).startswith(str(ROOT) + "/") for l in layers) and not any(
        "/tmp/" in l for l in layers) and len([l for l in layers if not l.endswith("regression.yaml")]) <= 3


def latest_results() -> dict[str, tuple[str, dict]]:
    """game id -> (run date, result) of its latest plain run."""
    latest = {}
    for run in sorted(p for p in RESULTS.glob("*") if (p / "results.json").is_file() and not (p / "experiment").exists()):
        for rel, res in json.loads((run / "results.json").read_text()).items():
            gid = Path(rel).stem
            if "skipped" in res or not plain_run(run, gid):
                continue
            latest[gid] = (run.name[:8], res)
    return latest


def result_cell(res: dict) -> str:
    failed = [c for c in CHECKS if res.get(c) is False]
    if not failed:
        return "pass"
    return "fail: " + ", ".join(failed)


def table() -> str:
    latest = latest_results()
    lines = ["| Game id | Game | System | Status | Regression test | Run |", "|---|---|---|---|---|---|"]
    for g in doc_games():
        date, res = latest.get(g["id"], (None, None))
        test = result_cell(res) if res else "not in the test"
        run = f"{date[:4]}-{date[4:6]}-{date[6:]}" if date else ""
        lines.append(f"| `{g['id']}` | {g['game']} | {g['system']} | {g['result']} | {test} | {run} |")
    return "\n".join(lines)


def main():
    text = README.read_text()
    start, end = text.index(BEGIN) + len(BEGIN), text.index(END)
    README.write_text(text[:start] + "\n" + table() + "\n" + text[end:])
    print(f"{README}: {len(doc_games())} games")


if __name__ == "__main__":
    main()
