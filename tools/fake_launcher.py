#!/usr/bin/env python3
"""Stands in for arcade-launcher on the TCP port to test a payload without real inputs.

Run it, then start the game with the launcher's environment (see docs/IMPLEMENTATION.md,
"Testing a payload without the launcher"). It sends a scripted sequence of virtual stick
states for player 1; check the payload log for the native values the game read.
"""
import socket
import struct
import sys
import time

# Virtual stick bits (crates/protocol/src/lib.rs, mod button)
UP, DOWN, LEFT, RIGHT = 1 << 0, 1 << 1, 1 << 2, 1 << 3
B = {i: 1 << (3 + i) for i in range(1, 9)}  # B[1]..B[8]
START, COIN, SERVICE, TEST = 1 << 12, 1 << 13, 1 << 14, 1 << 15

KIND_INPUT = 2
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 33700

# (seconds after the payload connects, player 1 buttons)
SCRIPT = [
    (15.0, COIN), (15.3, 0),
    (16.0, COIN), (16.3, 0),
    (18.0, START), (18.3, 0),
    (20.0, RIGHT), (20.5, 0),
    (21.0, DOWN | B[1]), (21.5, 0),
    (22.0, B[5]), (22.5, 0),
]


def frame(p1_buttons):
    # 4 players x {u32 buttons, 6 x i16 axes: lx ly rx ry accel brake}
    players = [struct.pack('<I6h', p1_buttons, 0, 0, 0, 0, 0, 0)]
    players += [struct.pack('<I6h', 0, 0, 0, 0, 0, 0, 0)] * 3
    payload = b''.join(players)
    return struct.pack('<BBH', KIND_INPUT, 0, len(payload)) + payload


def main():
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(('127.0.0.1', PORT))
    srv.listen(1)
    conn, _ = srv.accept()
    print('payload connected', flush=True)
    t0 = time.time()
    for at, buttons in SCRIPT:
        time.sleep(max(0.0, at - (time.time() - t0)))
        conn.sendall(frame(buttons))
        print(f'{at:5.1f}s sent {buttons:#06x}', flush=True)
    time.sleep(5)


if __name__ == '__main__':
    main()
