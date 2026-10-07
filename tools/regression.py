#!/usr/bin/env python3
"""Regression test of the working games: launches each game of tools/regression/games.yaml in
turn, then checks that it started (payload connected), is still running, shows a window
(viewable X11 window of its processes, 200x200 at least, XWayland too), shows a picture
(Direct3D 8/9 screenshots, WAL_SCREENSHOT) and plays sound (its PulseAudio/PipeWire stream).

Usage: tools/regression.py [--wait S] [--only TEXT]... [--compare RUN_DIR] [--no-prepare]
                           [--root DIR] [--label NAME] [--profile FILE]...
Results: tools/regression/results/<date>/ (report.md, results.json, logs and screenshots per
game), compared with the previous run: a check that passed before and fails now is a
regression. Run ./build.sh first. One game at a time (input port 33700): close running games.
"""

import argparse
import ctypes
import ctypes.util
import json
import os
import re
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
# launcher tree (--root: another checkout, e.g. the previous commit for a reference run)
TREE = ROOT
GAMES = ROOT / "games"
CONF = ROOT / "tools/regression/games.yaml"
RESULTS = ROOT / "tools/regression/results"
PORT = 33700
# coin + start after 30 s (per game: script: <file> | none)
DEFAULT_SCRIPT = "tools/regression/coin-start.txt"
CHECKS = ["started", "running", "window", "picture", "audio"]


def port_busy() -> bool:
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", PORT)) == 0


def wait_port_free(timeout: float) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        if not port_busy():
            return True
        time.sleep(1)
    return False


def windows_path(p: Path) -> str:
    return "Z:" + str(p).replace("/", "\\")


def bmp_stats(path: Path):
    """(distinct colors in a sample, share of non-black pixels) of a 24/32-bit BMP."""
    data = path.read_bytes()
    off, = struct.unpack_from("<I", data, 10)
    w, h, _, bpp = struct.unpack_from("<iiHH", data, 18)
    h = abs(h)
    bpp //= 8
    stride = (w * bpp + 3) & ~3
    colors, lit, n = set(), 0, 0
    for y in range(0, h, max(1, h // 64)):
        row = off + y * stride
        for x in range(0, w, max(1, w // 64)):
            px = data[row + x * bpp: row + x * bpp + 3]
            colors.add(px)
            lit += max(px) > 16
            n += 1
    return len(colors), lit / max(n, 1)


def game_streams() -> list[dict]:
    """Sink inputs of Windows programs (wine's winepulse)."""
    try:
        out = subprocess.run(["pactl", "-f", "json", "list", "sink-inputs"], capture_output=True, text=True, timeout=10).stdout
        inputs = json.loads(out or "[]")
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError):
        return []
    found = []
    for si in inputs:
        props = si.get("properties", {})
        text = " ".join(str(props.get(k, "")) for k in ("application.process.binary", "application.name", "media.name")).lower()
        # playing ones only (corked: paused, or wine's format test stream)
        if (".exe" in text or "wine" in text or "preloader" in text) and not si.get("corked"):
            found.append(si)
    return found


CAPTURE_SINK = "wal_regression"


def pactl(*args: str) -> str:
    try:
        return subprocess.run(["pactl", *args], capture_output=True, text=True, errors="replace", timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def capture_sink_setup() -> list[str]:
    """A null sink for the games' streams, looped back to the default sink (still heard): its
    recording holds the game's sound only. Returns the pactl module ids to unload."""
    default = pactl("get-default-sink")
    null = pactl("load-module", "module-null-sink", f"sink_name={CAPTURE_SINK}",
                 "sink_properties=device.description=WAL-regression")
    if not null.isdigit():
        return []
    loop = pactl("load-module", "module-loopback", f"source={CAPTURE_SINK}.monitor", f"sink={default}", "latency_msec=60")
    return [m for m in (loop, null) if m.isdigit()]


def capture_sink_teardown(modules: list[str]):
    for m in modules:
        pactl("unload-module", m)


def sink_names() -> dict[int, str]:
    try:
        out = subprocess.run(["pactl", "-f", "json", "list", "sinks"], capture_output=True, text=True, errors="replace", timeout=10).stdout
        return {s["index"]: s["name"] for s in json.loads(out or "[]")}
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError):
        return {}


def sink_peak(sink: str, seconds: float = 2.0) -> float:
    """Peak (0..1) of what a sink plays over a few seconds. The per-stream monitor
    (parec --monitor-stream) reads silence for wine's streams: the sink is recorded instead
    (other programs playing at the same time count too)."""
    with tempfile.NamedTemporaryFile(suffix=".raw") as f:
        try:
            subprocess.run(
                ["timeout", str(seconds), "pw-record", "--target", sink, "-P", "{ stream.capture.sink=true }",
                 "--format", "f32", "--rate", "48000", "--channels", "2", "--raw", f.name],
                capture_output=True, timeout=seconds + 5)
        except (OSError, subprocess.SubprocessError):
            return 0.0
        data = Path(f.name).read_bytes()
    data = data[: len(data) // 4 * 4]
    if not data:
        return 0.0
    return max(abs(v) for v in struct.unpack(f"<{len(data) // 4}f", data))


class XWindowAttributes(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int), ("height", ctypes.c_int),
                ("border_width", ctypes.c_int), ("depth", ctypes.c_int), ("visual", ctypes.c_void_p),
                ("root", ctypes.c_ulong), ("c_class", ctypes.c_int), ("bit_gravity", ctypes.c_int),
                ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int), ("backing_planes", ctypes.c_ulong),
                ("backing_pixel", ctypes.c_ulong), ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong),
                ("map_installed", ctypes.c_int), ("map_state", ctypes.c_int), ("all_event_masks", ctypes.c_long),
                ("your_event_mask", ctypes.c_long), ("do_not_propagate_mask", ctypes.c_long),
                ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p)]


def wine_pids() -> set[int]:
    """Wine processes (WINEPREFIX in their environment): the game's, one game at a time. Not
    the launcher's session: the game, wineserver and explorer.exe get sessions of their own."""
    pids = set()
    for p in os.listdir("/proc"):
        try:
            if p.isdigit() and b"\0WINEPREFIX=" in b"\0" + Path(f"/proc/{p}/environ").read_bytes():
                pids.add(int(p))
        except OSError:
            pass
    return pids


def in_gamescope(sid: int) -> bool:
    """A gamescope process in the launcher's session (the game runs inside it)."""
    for p in os.listdir("/proc"):
        try:
            if p.isdigit() and os.getsid(int(p)) == sid and Path(f"/proc/{p}/comm").read_text().strip() == "gamescope":
                return True
        except OSError:
            pass
    return False


def game_windows(pids: set[int]) -> list[tuple[int, int]] | None:
    """(width, height) of the viewable windows (window manager's client list) of these
    processes, from the X server (XWayland too). None: no X display to ask."""
    try:
        x = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    except OSError:
        return None
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XDefaultRootWindow.restype = ctypes.c_ulong
    x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x.XInternAtom.restype = ctypes.c_ulong
    x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x.XGetWindowProperty.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_long, ctypes.c_long,
                                     ctypes.c_int, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                                     ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_ulong),
                                     ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_void_p)]
    x.XGetWindowAttributes.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(XWindowAttributes)]
    x.XFree.argtypes = [ctypes.c_void_p]
    x.XCloseDisplay.argtypes = [ctypes.c_void_p]
    dpy = x.XOpenDisplay(None)
    if not dpy:
        return None

    def prop(win: int, name: bytes) -> list[int]:
        typ, fmt, n, after, data = ctypes.c_ulong(), ctypes.c_int(), ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_void_p()
        atom = x.XInternAtom(dpy, name, 0)
        if x.XGetWindowProperty(dpy, win, atom, 0, 4096, 0, 0, ctypes.byref(typ), ctypes.byref(fmt),
                                ctypes.byref(n), ctypes.byref(after), ctypes.byref(data)) != 0 or not data:
            return []
        # format 32 items are C longs
        values = list(ctypes.cast(data, ctypes.POINTER(ctypes.c_ulong))[: n.value]) if fmt.value == 32 else []
        x.XFree(data)
        return values

    found = []
    try:
        for win in prop(x.XDefaultRootWindow(dpy), b"_NET_CLIENT_LIST"):
            pid = prop(win, b"_NET_WM_PID")
            attrs = XWindowAttributes()
            if pid and pid[0] in pids and x.XGetWindowAttributes(dpy, win, ctypes.byref(attrs)) \
                    and attrs.map_state == 2:  # IsViewable
                found.append((attrs.width, attrs.height))
    finally:
        x.XCloseDisplay(dpy)
    return found


def stop(proc: subprocess.Popen):
    """The launcher stops the game (wineserver -k); its whole session is killed if it hangs."""
    if proc.poll() is None:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(30)
        except subprocess.TimeoutExpired:
            pass
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    proc.wait()
    wait_port_free(30)


def run_game(rel: str, opts: dict, wait: float, out: Path, layers: list[Path]) -> dict:
    dump = GAMES / rel
    out.mkdir(parents=True, exist_ok=True)
    shots = out / "shots"
    shots.mkdir(exist_ok=True)
    layer = out / "regression.yaml"
    layer.write_text(yaml.safe_dump({"env": {"WAL_SCREENSHOT": "5", "WAL_SCREENSHOT_DIR": windows_path(shots)}}))
    cmd = [str(TREE / "dist/arcade-launcher"), "run", str(dump), "--profile", str(layer)]
    for extra in layers:
        cmd += ["--profile", str(extra.resolve())]
    script = opts.get("script", DEFAULT_SCRIPT)
    if script and script != "none":
        cmd += ["--input-script", str(ROOT / script)]
    log = open(out / "launcher.log", "w")
    start = time.time()
    proc = subprocess.Popen(cmd, cwd=TREE, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    res = {"dump": rel}
    # the audio stream shows up any time (some games start their sound late): sample the
    # playing streams during the whole run
    peak, streams = 0.0, 0
    while time.time() - start < wait and proc.poll() is None:
        found = game_streams()
        streams = max(streams, len(found))
        if found:
            sinks = sink_names()
            if CAPTURE_SINK in sinks.values():
                # the game's streams alone in the capture sink
                for si in found:
                    if sinks.get(si.get("sink")) != CAPTURE_SINK:
                        pactl("move-sink-input", str(si["index"]), CAPTURE_SINK)
                peak = max(peak, sink_peak(CAPTURE_SINK))
            else:
                peak = max([peak] + [sink_peak(sinks[si["sink"]]) for si in found if si.get("sink") in sinks])
        else:
            time.sleep(2)
    res["seconds"] = round(time.time() - start)
    res["running"] = proc.poll() is None
    # at the end (startup windows, launchers and loading screens are gone by then)
    windows = game_windows(wine_pids()) if res["running"] else []
    if res["running"] and in_gamescope(proc.pid):
        # its own nested X server, shown as a Wayland window: not visible here
        res["window"] = None
        res["window_detail"] = "gamescope"
    elif windows is None:
        res["window"] = None
        res["window_detail"] = "no X display"
    else:
        big = [w for w in windows if w[0] >= 200 and w[1] >= 200]
        res["window"] = bool(big)
        res["window_detail"] = ", ".join(f"{w}x{h}" for w, h in windows) or "no window"
    log.flush()
    text = (out / "launcher.log").read_text(errors="replace")
    m = re.search(r"^run directory: (.+)$", text, re.M)
    payload_log = None
    if m:
        logs = list(Path(m.group(1)).glob("wal-*.log"))
        if logs:
            payload_log = logs[0]
            (out / payload_log.name).write_text(payload_log.read_text(errors="replace"))
    plog = (out / payload_log.name).read_text(errors="replace") if payload_log else ""
    res["started"] = "connected to launcher" in plog
    shot_files = sorted(shots.glob("*.bmp"))
    if shot_files:
        # attract modes flash, fade and blank: the best of the later shots
        stats = [bmp_stats(f) for f in shot_files[len(shot_files) // 3:]]
        colors, lit = max(stats)
        res["picture"] = colors >= 16 and lit > 0.02
        res["picture_detail"] = f"{len(shot_files)} shots, best: {colors} colors, {lit:.0%} lit"
    else:
        res["picture"] = None  # not Direct3D 8/9 (or no frame): not checked
        res["picture_detail"] = "no screenshot"
    res["audio"] = peak > 0.01
    res["audio_detail"] = f"{streams} stream(s), peak {peak:.3f}"
    stop(proc)
    log.close()
    if not res["running"]:
        res["exit"] = "\n".join(text.splitlines()[-5:])
    return res


def latest_run(exclude: Path) -> Path | None:
    runs = sorted(p for p in RESULTS.glob("*") if (p / "results.json").is_file() and p != exclude)
    return runs[-1] if runs else None


def mark(v) -> str:
    return {True: "ok", False: "FAIL", None: "-"}[v]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--wait", type=float, default=60, help="seconds per game before the checks (default 60)")
    ap.add_argument("--only", action="append", default=[], help="games whose dump path contains TEXT")
    ap.add_argument("--compare", type=Path, help="run directory to compare with (default: the previous run)")
    ap.add_argument("--no-prepare", action="store_true", help="do not prepare the prefix first")
    ap.add_argument("--root", type=Path, help="launcher tree to test (default: this checkout)")
    ap.add_argument("--profile", type=Path, action="append", default=[], help="extra launcher profile layer (experiments)")
    ap.add_argument("--label", default="", help="suffix of the results directory name")
    args = ap.parse_args()

    global TREE
    TREE = (args.root or ROOT).resolve()
    if not (TREE / "dist/arcade-launcher").is_file():
        sys.exit("dist/arcade-launcher missing: run ./build.sh")
    if port_busy():
        sys.exit(f"port {PORT} busy: a game is running")
    games = yaml.safe_load(CONF.read_text()) or {}
    games = {k: v or {} for k, v in games.items() if not args.only or any(o.lower() in k.lower() for o in args.only)}
    run_dir = RESULTS / (time.strftime("%Y%m%d-%H%M%S") + (f"-{args.label}" if args.label else ""))
    run_dir.mkdir(parents=True)
    if args.profile:
        # not a reference result (tools/gamelist.py skips it)
        (run_dir / "experiment").write_text("".join(f"--profile {p}\n" for p in args.profile))

    if not args.no_prepare and games:
        # prefix creation and winetricks downloads, once, before the timed runs
        first = GAMES / next(iter(games))
        print(f"preparing the prefix ({first.name})...", flush=True)
        with open(run_dir / "prepare.log", "w") as log:
            if subprocess.run([str(TREE / "dist/arcade-launcher"), "prepare", str(first)], cwd=TREE, stdout=log, stderr=subprocess.STDOUT).returncode:
                sys.exit(f"prefix preparation failed: {run_dir / 'prepare.log'}")

    modules = capture_sink_setup()
    if not modules:
        print("warning: no capture sink (pactl load-module failed): audio measured on the real sink, other programs count too")
    # a locked/blanked screen stops the games' frames and sound: no idle while testing
    inhibit = None
    for cmd in (["gnome-session-inhibit", "--inhibit", "idle", "--inhibit-only"],
                ["systemd-inhibit", "--what=idle", "--why=regression test", "sleep", "infinity"]):
        try:
            inhibit = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            break
        except OSError:
            pass
    results = {}
    try:
        run_games(games, args, run_dir, results)
    finally:
        capture_sink_teardown(modules)
        if inhibit:
            inhibit.terminate()
    report(results, args, run_dir)


def run_games(games: dict, args, run_dir: Path, results: dict):
    for i, (rel, opts) in enumerate(games.items(), 1):
        if opts.get("skip"):
            results[rel] = {"dump": rel, "skipped": opts["skip"]}
            continue
        name = Path(rel).stem
        print(f"[{i}/{len(games)}] {rel} ...", end=" ", flush=True)
        res = run_game(rel, opts, opts.get("wait", args.wait), run_dir / name, args.profile)
        results[rel] = res
        print(" ".join(f"{c}={mark(res.get(c))}" for c in CHECKS), flush=True)
        (run_dir / "results.json").write_text(json.dumps(results, indent=1))


def report(results: dict, args, run_dir: Path):
    prev_dir = args.compare or latest_run(run_dir)
    prev = json.loads((prev_dir / "results.json").read_text()) if prev_dir else {}
    lines = [f"# Regression run {run_dir.name}", "",
             f"Compared with: {prev_dir.name if prev_dir else 'nothing'}", "",
             "| game | started | running | window | picture | audio | regressions | details |", "|---|---|---|---|---|---|---|---|"]
    regressions = 0
    for rel, res in results.items():
        if "skipped" in res:
            lines.append(f"| {rel} | skipped: {res['skipped']} | | | | | | |")
            continue
        old = prev.get(rel, {})
        lost = [c for c in CHECKS if old.get(c) is True and res.get(c) is not True]
        regressions += bool(lost)
        details = f"{res.get('window_detail', '')}; {res.get('picture_detail', '')}; {res.get('audio_detail', '')}"
        lines.append(f"| {rel} | " + " | ".join(mark(res.get(c)) for c in CHECKS) + f" | {', '.join(lost) or ''} | {details} |")
    lines += ["", f"{regressions} game(s) with regressions."]
    (run_dir / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nresults: {run_dir}")
    sys.exit(1 if regressions else 0)


if __name__ == "__main__":
    main()
