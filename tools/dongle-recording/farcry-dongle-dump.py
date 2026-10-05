#!/usr/bin/env python3
# Reads Far Cry Paradise Lost's dongle data from the running game (FarCry_r.exe, MD5
# 87648806d0b4b5a5384e7eedf4882d7e) on a setup where the HASP4 dongle answers, without
# touching the executable or its DLLs. Run as root (or the game's user) on that machine,
# then start a level: the game reads the dongle at level start.
#
# The game keeps bytes 0x00..0x13 of the dongle memory in a struct at 0xa5278c:
#   +0x00 valid | +0x01 game id (6) | +0x08 version (3) | +0x0c region (2) |
#   +0x10 country (4) | +0x15 cabinet type (3) | +0x19 cabinet (1)
# and its "IncorrectDongle" flag at 0x9d7630. The payload's dongle memory
# (crates/payload-globalvr/src/hasp.rs) is rebuilt from that struct.
import os
import time

STRUCT, ERROR = 0xa5278c, 0x9d7630
# struct offset -> dongle memory offset, length
FIELDS = [(0x01, 0x00, 6), (0x08, 0x06, 3), (0x10, 0x09, 4), (0x0c, 0x0d, 2), (0x19, 0x0f, 1), (0x15, 0x10, 3)]


def games():
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try:
            if b'FarCry_r.exe' in open(f'/proc/{pid}/cmdline', 'rb').read():
                yield pid
        except OSError:
            pass


last = None
while True:
    for pid in games():
        try:
            with open(f'/proc/{pid}/mem', 'rb') as mem:
                mem.seek(0x400000)
                if mem.read(2) != b'MZ':
                    continue  # the launcher, not the game
                mem.seek(STRUCT)
                s = mem.read(0x1b)
                mem.seek(ERROR)
                err = mem.read(1)[0]
        except OSError:
            continue
        if s != last:
            last = s
            memory = bytearray(0x13)
            for src, dst, n in FIELDS:
                memory[dst:dst + n] = s[src:src + n]
            print(f'pid {pid} valid {s[0]} dongle error {err} memory {bytes(memory)!r}', flush=True)
    time.sleep(1)
