#!/bin/sh
# Builds FarCry_r_hasplog.exe: a copy of Far Cry Paradise Lost's FarCry_r.exe (build
# MD5 87648806d0b4b5a5384e7eedf4882d7e) whose hasp() calls go through
# farcry-hasp-cave.S, logging every call and its answer to hasplog.bin. Run that copy
# under an existing dongle emulation (in place of FarCry_r.exe) to record what it
# answers, then build the payload's dongle image from the log.
#
# usage: farcry-hasp-record.sh <FarCry_r.exe> [output]
set -e
src=$1
out=${2:-$(dirname "$src")/FarCry_r_hasplog.exe}
here=$(dirname "$0")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

i686-w64-mingw32-as "$here/farcry-hasp-cave.S" -o "$tmp/cave.o"
i686-w64-mingw32-ld -Ttext=0x8f96b0 --image-base=0 -e _start -o "$tmp/cave.exe" "$tmp/cave.o"
i686-w64-mingw32-objcopy -O binary -j .text "$tmp/cave.exe" "$tmp/cave.bin"

python3 - "$src" "$out" "$tmp/cave.bin" <<'PY'
import hashlib, struct, sys
src, out, cave = sys.argv[1], sys.argv[2], open(sys.argv[3], 'rb').read()
d = bytearray(open(src, 'rb').read())
if hashlib.md5(d).hexdigest() != '87648806d0b4b5a5384e7eedf4882d7e':
    sys.exit('not the expected FarCry_r.exe (MD5 87648806d0b4b5a5384e7eedf4882d7e)')
BASE, TEXT_VA, TEXT_RAW, TEXT_RAWSIZE = 0x400000, 0x1000, 0x400, 0x4f8800
off = lambda va: va - BASE - TEXT_VA + TEXT_RAW
CAVE, HASP = 0x8f96b0, 0x8a9094
assert len(cave) <= TEXT_RAW + TEXT_RAWSIZE - off(CAVE), 'cave too big'
assert not any(d[off(CAVE):off(CAVE) + len(cave)]), 'cave area not free'
d[off(CAVE):off(CAVE) + len(cave)] = cave
# .text VirtualSize -> its raw size, so the cave is part of the mapped section
pe = struct.unpack_from('<I', d, 0x3c)[0]
sec = pe + 24 + struct.unpack_from('<H', d, pe + 20)[0]
assert d[sec:sec + 5] == b'.text'
struct.pack_into('<I', d, sec + 8, TEXT_RAWSIZE)
for site in (0x8a8def, 0x8a8e1e, 0x8a8e76, 0x8a8f60):
    assert d[off(site)] == 0xE8 and site + 5 + struct.unpack_from('<i', d, off(site) + 1)[0] == HASP
    struct.pack_into('<i', d, off(site) + 1, CAVE - (site + 5))
open(out, 'wb').write(d)
print(f'{out}: cave {len(cave)} bytes at {CAVE:#x}, 4 hasp() calls repointed')
PY
