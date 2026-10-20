# Pleasant Goat and Big Big Wolf Happy Happy Playground — investigation handoff

Updated: **2026-10-21**. Investigation resumed. **Title/attract screens now verified.**
Coin insertion is verified (displayed credits increase). Gameplay, audio, payout,
and two-player controls remain unverified.

## Current result (supersedes the older investigation below)

The normal system profile now includes all settings needed to pass the startup checks:

- JVS capabilities remain 16 outputs, one player with 22 switches.
- `WAL_TYPEX_JVS_LAYOUT=happy-playground` asserts switch byte 3 bit `0x40`.
  Function `0x412050` requires this dispenser-ready signal; absent it, the game sets
  global error `7` at `0x41208d`. `node+0x148` is the third switch byte; its current
  and previous values are at `node+0x148` and `node+0x149`.
- The board EEPROM region is `04`; keychip region is `04`, model type `02`.
  `0x411f90` rejects the defaults (region `01`, model `00`) with error `2`.
  Accepted combinations are board/keychip region `08` with model `02`, or
  board/keychip region `04` with model `02` or `05`. Region `04`/model `02`
  produces English screens in the observed run.
- `WAL_PATCHES` redirects `CounterInfo.Dat` to `sv\CounterInfo.Dat`.
- COM3 uses the existing MicroTouch emulator; `01 5A 0D` receives `01 30 0D`.
  This did not itself clear the dispenser error; it is separate from COM2.

COM2 command `61` is implemented experimentally. Each channel has a stable,
process-local 1024-byte memory image, with a 51-byte synthetic record containing
100 cards at record offset `0x13`. The game's pack routine at `0x5ad1b9`
returns `1` and encodes it to 64 bytes at memory address `0x40`.
Transport encryption uses the existing `0x5af180` transform and the request key.
**Correction:** the header is read from address **`0x20`, length `0x10`**;
it is not a 32-byte read from address zero. Header `BVG`, region, and distinct
channel identifiers pass startup validation. Command `64` currently returns an
empty success acknowledgement; its writes are not persisted or verified.

Runtime evidence with tracing enabled:

```text
counter init result=1 error=4294967295
channel 1 state=ffffffff, id=00000001, stock=0064
channel 2 state=ffffffff, id=00000002, stock=0064
startup error=0xffffffff switches=[00,00,00,00,00,00,00,00,40,40,...]
```

Screenshots show the Sega presentation and game title/attract content. The
opposed portrait views are expected. The input-script-only sensor experiment
was replaced by the dedicated JVS layout; no held virtual button is required.
The game boots to attract mode. Its dual-screen output is composed into one
screen, with the main screen flipped and displayed at the bottom.

The normal profile was launched without `/tmp/happy-playground-probe.yaml` or
a scripted sensor override and reached the title screen. The regression coin
pulse increased displayed credits; default start pulses did not leave the title.

An additional input sweep established that third switch byte bit `0x80`
(`btn9`) requests the separate `TestMode.exe -t` service menu (foreground app 3).
Do not map this to a gameplay button. The experimental counter handler is now
restricted to `XYY_RingWide.exe`, since the test executable has different code
addresses. The test menu itself displays successfully.

Remaining work: verify start and player controls, determine the two-player
packing into one JVS record, implement and verify counter writes and payout
feedback, and replace executable-specific encoding with an independent codec
if practical. The counter implementation is still experimental and specific to
the executable hash recorded below. No commits have been made.

## Historical handoff (2026-10-20; superseded above)

## Exact executable and working files

- Game root: `games/ringwide/Pleasant Goat and Big Big Wolf Happy Happy Playground/`
- Executable: `exe/XYY_RingWide.exe`; PE image base `0x400000`.
- SHA-256: `90a43577d6a732eb00ee9a1a88e4f42b5b39b66166bfa034cc6d5d8844758827`.
- Game ID: `SBVG`; RingWide platform ID: `AAM`.
- Profile: `systemprofiles/ringedge/pleasant-goat-and-big-big-wolf-happy-happy-playground.yaml`.
- Experimental counter emulator: `crates/payload-ringedge/src/happy_playground.rs`.
- Payload initialization calls `happy_playground::init()` in `crates/payload-ringedge/src/lib.rs`.
- Other modified sources: `crates/payload-common/src/serial.rs` (per-port silent handles and
  tracing), `crates/payload-ringedge/src/amlog.rs` (temporary JVS diagnostics).
- Latest built DLL: `dist/payloads/wal_ringedge.dll`, copied from
  `target/i686-pc-windows-gnu/release/wal_ringedge.dll`.

These changes are still experimental/uncommitted. This handoff only updates this document;
no additional implementation was made after the pause request.

## Confirmed fix: JVS board capabilities

The game accepts this profile combination:

```yaml
env:
  WAL_TYPEX_JVS_OUTPUTS: "16"
  WAL_TYPEX_JVS_SWITCHES: "1x22"
```

Actual capability checks at VMA `0x411dfa`, `0x411e03`, and `0x411e0c` require:

- Communication version `0x30`.
- At least 16 general outputs.
- Exactly one switch-input player.
- 22 or 14 switch bits for that player.

The previous 2-player/16-button board configuration is rejected. A two-person cabinet does
not imply that its JVS board advertises two players; its physical controls may be packed into
one 22-bit input record. The mapping to the loader's two logical players remains unverified.

Runtime node base: `0x6b023c`. The fields read by the game include `node+0x104` (players),
`node+0x105` (switch bits), `node+0x128` (outputs), and `node+0x139` (comm version).
The old blanket assertion that board features are only at `node+0x2a8` was misleading.
No change to the shared JVS feature-record encoding was needed for this fix.

After the profile correction, polling is `20 01 03 21 02 32 01 00`: one player, three switch
bytes, two coin counters, one output byte. The screen changes from the JVS error to a card
error. Earlier screenshots read `吐卡机通信错误 / 请找服务人员` (card dispenser communication
error / contact staff).

Screenshots are **1024×1536**, with two opposed views for the cabinet. Do not treat the
portrait buffer as a rendering failure merely because `mxGetHwInfo.ini` mentions 1024×768.

## Confirmed fix: card counter save path

After its COM2 probe succeeds, startup calls `0x5a9d99` with this hard-coded filename:

```text
C:\Documents and Settings\AppUser\CounterInfo.Dat
```

String VMA: `0x65bd88`; RVA: `0x25bd88`. The Wine prefix has no `Documents and Settings`
parent directory. The helper attempts a single-level `CreateDirectoryA` and returns an error
on failure. The caller at `0x412604` routes this to the same global card error (`0x0d`) used
for a failed serial probe. Thus that screen alone does not prove a communication failure.

A temporary profile patch redirects the filename to `sv\CounterInfo.Dat`:

```yaml
env:
  WAL_PATCHES: "25bd88:73765c436f756e746572496e666f2e44617400"
```

**This patch is in `/tmp/happy-playground-probe.yaml`, not in the system profile.** It must be
included when resuming. With it, the game passes the initial probe and reaches additional
COM2 commands, including the current failing memory read. A final implementation should
keep the file in the game's save folder and restrict executable-specific patches appropriately.

## COM2 protocol: what has actually been verified

COM2 is opened directly by the game. This is a separate protocol from the Sanwa magnetic
reader in `card.rs`; `picodaemon.exe` is not involved in the observed startup exchange.

Requests are framed as:

```text
AA command 00 length payload... xor
```

Total request length is `request[3] + 5`. XOR of the full frame, including its checksum, is
zero. The game writes individual bytes through `WriteFile`, so the handler must accumulate
fragments before responding. Do not restart a frame when `AA` occurs inside its payload or
checksum: later requests contain random key bytes that may take any value. The current
handler has been corrected to look for `AA` only while waiting for a frame.

Responses parsed by `0x5ab910` are:

```text
55 status byte2 length payload... xor
```

The meaning of response byte 2 has not been established; the emulator currently uses zero.
The parser reads `length+5` bytes and checks that their XOR is zero.

The first request is:

```text
AA B0 00 07 80 00 00 00 00 00 08 95
```

An empty success response `55 00 00 00 55` **does** pass `0x5ab910` and `0x5aa490`, as
confirmed by diagnostic wrappers returning result `0`. It is insufficient for later code,
which parses the returned text using underscore separators. The emulator now supplies
`WAL_1_1\0` (8 bytes), producing:

```text
55 00 00 08 57 41 4C 5F 31 5F 31 00 07
```

`WAL_1_1` is an emulated identifier, not a captured hardware version. Both startup `B0`
queries return result zero and the expected eight output bytes in the latest run.

### Implemented commands and current failure

| Command | Current emulation / evidence |
|---------|------------------------------|
| `B0` | Success, 8-byte text `WAL_1_1\0`; probe and version parsing advance. |
| `B9` | Status **3**, representing both counter channels present to `0x5a93ff`. This helper interprets the response status byte itself as the channel bitmap, not ordinary success/failure. |
| `67` | Returns encrypted one-byte value `1`; this advances the counter-presence check on channels 1 and 2. |
| `B2` | Empty success; observed after `67` and accepted sufficiently to reach `61`. |
| `B3`, `B4`, `C0` | Empty success placeholders. `C0` is observed after a read timeout; complete semantics have not been verified. |
| **`61`** | **Not implemented. The latest run stops making startup progress here.** |

Latest runtime sequence, copied here because the payload log is overwritten on launch:

```text
B0 -> parsed result 0, data 57 41 4C 5F 31 5F 31 00
B0 -> parsed result 0, same data
AA B9 00 05 80 00 00 00 00 96                         supported=true
AA 67 00 07 01 00 00 00 00 37 3D C1                   supported=true
AA B2 00 05 01 00 00 00 00 1C                         supported=true
AA 61 00 09 01 00 00 00 40 08 78 00 40 B3             supported=false
# about 8 seconds pass with no response to 61
AA C0 00 07 80 00 00 00 00 03 00 EE                   supported=true
AA B9 00 05 80 00 00 00 00 96                         supported=true
AA 67 00 07 02 00 00 00 00 73 74 CF                   supported=true
AA B2 00 05 02 00 00 00 00 1F                         supported=true
AA 61 00 09 02 00 00 00 40 8A 70 00 40 3A             supported=false
```

The `61` command reads channel 1 or 2 (`request[4]`), address `0x0040` (bytes 7–8,
big endian), length `0x40` (byte 12). Bytes 9–10 are a two-byte key. The next implementation
step is to answer this read with a valid encoded 64-byte counter record, with transport
encoding derived from that key. These field interpretations come from disassembly; a
successful `61` response has not yet been tested.

## Counter records and crypto: useful static findings, not a completed implementation

Startup function `0x4125b0` calls:

1. `0x5a9b1a(1)` to open COM2 and query `B0`.
2. `0x5a9d99(filename)` to prepare the counter save file.
3. `0x5a9c52(...)` to query `B0` again and parse its underscore-separated version numbers.
4. For each channel, `0x5a9b8b` / `0x5a993f` to read/decode a counter record.
5. `0x5a9708` to read a separate 32-byte header, then check game/region identifiers and stock.

Relevant functions (all addresses below are **VMAs**, subtract `0x400000` for RVAs):

| Address | Role / next use |
|---------|-----------------|
| `0x5aa490` | `B0` query, stdcall `(handle, length, output)`, `ret 0x0c`; expected length 8. |
| `0x5ab910` | Response frame parser, stdcall `(handle, output)`, `ret 8`. |
| `0x5aab10` / `0x5aab30` | XOR generation / zero-XOR validation. |
| `0x5aac90` / `0x5aab60` | Read initial byte / subsequent byte, with ~8 s / ~360 ms timeouts. |
| `0x5aa6e0` / `0x5a93ff` | `B9` presence bitmap; channel 1 accepts 2 or 3, channel 2 accepts 1 or 3. |
| `0x5accd0` | `67` query; decrypts one response byte with request bytes 9–10 as key. |
| `0x5aa790` / `0x5aa840` | `B2` / `B3`, channel 1 or 2. |
| `0x5abe90` | `61` memory reads, split into chunks of at most 80 bytes; decrypts replies. |
| `0x5aca50` | `64` command, associated with writing a two-byte counter value. |
| `0x5af180` | Transport transform: cdecl `(length, input, output, two_byte_key)`. Used successfully by the emulator for `67`. |
| `0x5ad1b9` | Packs a record: cdecl `(record, record_length, encoded_output, auxiliary_output, auxiliary_length)`. Not yet called by the emulator. |
| `0x5ad2e5` | Inverse record decoding; called with encoded length 64, output record, auxiliary output length 2. |

The transport transform appears to derive a 16-byte digest from the two-byte key and XOR
it repeatedly with the payload. The digest routines look like MD5, but that identification
has not been independently checked. The current prototype calls the game's own `0x5af180`
instead of reimplementing it. Its addresses are specific to the executable hash above.

The record pack routine takes a 51-byte record (`0x33`), adds five random bytes, transforms
that 56-byte body and appends an eight-byte key, yielding 64 bytes. Its auxiliary output of
length 2 appears to contain trailing bytes of the random data. This is a disassembly-derived
model, not a verified synthetic counter record. A possible next approach is to use the
existing pack routine to create records for a virtual counter, then replace such dependencies
with a standalone implementation if appropriate.

Further header/validation observations from `0x412688` onward:

- A separate raw header read is at address 0, length `0x20`.
- Header bytes 0–2 must be ASCII **`BVG`** (not `SBVG`).
- Header bytes 3–5 are compared to three bytes at `0x6a3e44 + region_index*3`.
- The region index comes from `0x404d60` -> `0x404a00`, which reads
  `(*(u32*)0x293b81c)+0xa8`.
- The decoded record's little-endian word at offset `0x13` is remaining card stock; the game
  requires it to be at most `0x030c` (780). Zero selects an empty-stock path.
- `0x404d80` / `0x404d90` / `0x404da0` lead to save-record matching and counter checks
  (`0x404700`, `0x404740`, `0x4047a0`). These checks also need to pass; replying with arbitrary
  zero-filled data is unlikely to be sufficient.

No synthetic stock records, persistent virtual stock, payout feedback, or write operations
have been implemented yet.

## COM3 and input status

COM3 remains configured as a silent port. The observed write is `01 5A 0D`. A worker polls
it with zero-byte reads, apparently based on the empty `ClearCommError` queue count.
Its role is unresolved. **Do not assume COM3 is the current blocker:** COM2 now demonstrably
stalls on `61` before card-counter initialization can finish.

Other potentially useful locations:

- `CCardOuput` (original spelling) RTTI type descriptor `0x6a2e5c`, vtable `0x65a2e8`.
- Generic serial open `0x5a62d0`; generic write `0x5a5f30`; worker read `0x5a5fa0`.
- `0x41b5ff` calls `0x5a7da0`, which opens the generic serial device with COM3 selected.
- `0x5a81d9` calls the generic serial writer.

`tools/regression/coin-start.txt` was used on earlier runs. Payload logs show coin (`P1
buttons 0x2000`) and start (`0x1000`) injection, and a backup update after coin. That does
**not** establish working in-game controls while startup is still on an error screen.
Two-player packing into the single JVS input record still needs investigation.

## Reproduce later

The system profile already includes JVS settings, platform/keychip configuration, screenshots
every two seconds, the game log hook, the experimental COM2 device, and silent COM3.
For the latest investigation state, recreate this additional profile:

```sh
cat > /tmp/happy-playground-probe.yaml <<'EOF'
env:
  WAL_RINGWIDE_HAPPY_PLAYGROUND_TRACE: "1"
  WAL_SERIAL_TRACE: "300"
  WAL_TRACE_FILES: "1"
  WAL_PATCHES: "25bd88:73765c436f756e746572496e666f2e44617400"
EOF
```

Build and copy the payload:

```sh
RUSTC=/home/aderumier/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc \
/home/aderumier/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo \
  build --release --target i686-pc-windows-gnu -p wal-payload-ringedge
cp target/i686-pc-windows-gnu/release/wal_ringedge.dll dist/payloads/wal_ringedge.dll
```

The latest build succeeded, with existing `dinput.rs` dead-code warnings and a MinGW
`DllMain` stdcall linker warning. No automated regression suite was run.

Launch (only when resuming the investigation):

```sh
timeout 30 dist/arcade-launcher run \
  "games/ringwide/Pleasant Goat and Big Big Wolf Happy Happy Playground" \
  --headless --no-input --profile /tmp/happy-playground-probe.yaml
```

The launcher needs graphics devices and its local TCP listener (`127.0.0.1:33700`), so it has
required elevated sandbox execution in this session. `timeout` status 124 is not proof of
successful boot: inspect protocol progress and screenshots.

Artifacts:

- Payload log: `wine-prefix/full/drive_c/wal/ringedge/pleasant-goat-and-big-big-wolf-happy-happy-playground/wal-ringedge.log`.
- Screenshots: the same run directory's `exe/shot-NNNN.bmp`.
- Full cached disassembly: `/tmp/opencode/hp-disasm.txt` (temporary; regenerate if missing).
- Focused extracts: `/tmp/happy-card-init.asm`, `/tmp/happy-card-output.asm` (temporary).
- Latest launch console: `/tmp/happy-playground-launch.log` (temporary).

Regenerate disassembly with:

```sh
objdump -M intel -d \
  "games/ringwide/Pleasant Goat and Big Big Wolf Happy Happy Playground/exe/XYY_RingWide.exe" \
  > /tmp/happy-playground-disasm.txt
```

PE offset conversion for this binary: `.text` file offset = VMA − `0x400000` − `0xc00`;
`.rdata` / `.data` use delta `0xe00`. See historical section table for section starts.

## Before declaring support complete

Implement and validate the `61` response and subsequent memory writes/counter validation;
then investigate any remaining COM3 requirement or card-output feedback. Verify sustained
attract playback, coin/start, both players' controls and a playable mini-game. TestMode has
not been verified. The prototype's crypto and diagnostic hooks use executable-specific
addresses, so they must not run against TestMode or another executable without validation.

The JVS dump hook in `amlog.rs` never emitted its intended return callback; the delayed node
dump did work. Remove the temporary hooks, profile screenshot/log verbosity, and other
investigation scaffolding before treating the profile as finished.


---

<details>
<summary>Historical notes — retained for reference, superseded by the findings above</summary>

# Pleasant Goat and Big Big Wolf — JVS "board spec" failure (6402)

Sega RingWide game, id **SBVG**, `exe/XYY_RingWide.exe` (image base `0x400000`).
Symptom: boots to `fs/bank/jvs_error.bnk` error screen (code 6402,
"JVS I/O 基板 不支持游戏规格" = "JVS I/O board does not support the game spec").
Payload connects, keychip PCP works, all files open, game stays alive (exit 124).

## Status: STUCK on locating the exact check that raises 6402

Everything up to the JVS spec acceptance works. The game loads `jvs_error.bnk`
~90 ms after the main poll loop starts, then polls forever on the error screen.

---

## PE / offset math (CORRECTED — earlier notes were wrong)

Image base `0x400000`. RVA = VMA − 0x400000. File offset:

| section | VMA     | file    | delta (file = RVA − delta) |
|---------|---------|---------|----------------------------|
| .text   | 0x1000  | 0x400   | **0xC00**                  |
| PSFD00  | 0x20c000| 0x20b200| 0xE00                      |
| .rdata  | 0x20e000| 0x20d200| 0xE00                      |
| .data   | 0x26d000| 0x26c200| 0xE00                      |

So e.g. VMA `0x5820b0` → RVA `0x1820b0` → file `0x1814b0`.
Helper (verified):
```python
def foff(vma):
    rva = vma - 0x400000
    return rva - 0xC00 if rva < 0x20B000 else rva - 0xE00
```
Full disasm cached at `/tmp/opencode/hp-disasm.txt` (`objdump -M intel -d XYY_RingWide.exe`, 796k lines).

---

## THE KEY FINDING: feature-record (`14`) parse contract

The game parses the JVS `14` (features) reply as a sequence of **4-byte records**
`[type, v1, v2, v3]`, **terminated by a record whose type byte is 0**.

Parser: `0x581ef0` (disasm ~line 595455). Loop at `0x581f70`, dispatch
`jmp [ecx*4 + 0x5820b0]` with `ecx = type − 1`. Jump table dumped (file `0x1814b0`),
21 entries. A record's type is read at the current offset; after each record
`off += 4` and the next type byte is checked (`0` → clean return 0; else continue;
`off >= 0x100` → return 0; `type > 21` → return −5).

Note: a **zero type byte is only legal as the terminator** (checked before dispatch).
The `table[0]` entry (`0x581f94`) is unreachable.

Valid types and the fields they write into the 0x34-byte feature struct
(dest = `node + 0x2a8`, reached via `0x583110(node, ?, node+0x1a8)` → features at
`+0x1a8+0x100 = node+0x2a8`; versions rev/jvs/comm at `node+0x2dc..de`):

| type | dest offset(s) | likely meaning          |
|------|----------------|--------------------------|
| 1    | [0], [1]       | nb_player, nb_button      |
| 2    | [4]            | nb_coin_counter           |
| 3    | [8], [9]       | ? (2 bytes)               |
| 4    | [0xc]          | ?                         |
| 5    | [0x10]=1 (DWORD flag) | ?                 |
| 6    | [0x14..0x16]   | ? (3 bytes)               |
| 7    | [0x18], [0x19] | ? (2 bytes)               |
| **8–15** | —          | **return −5 (unsupported)** |
| 16   | [0x1c]         | ?                         |
| 17   | [0x20]         | ?                         |
| 18   | [0x24]         | ? ← we currently put GP outputs here |
| 19   | [0x28]         | ?                         |
| 20   | [0x2c..0x2e]   | ? (3 bytes)               |
| 21   | [0x30]=1 (DWORD flag) | ?                 |

**The field meanings for types 3,4,6,7,16–21 are still unmapped** — this is the
crux. We need to know which type is **general-purpose outputs** (the game requires 1).

### Why the earlier "raw legacy blob" hypothesis was WRONG
Our `14` reply is *already* valid record format and parses cleanly:
- default (12 B): `01 02 10 00 | 02 02 00 00 | 00 00 00 00`
  → type1(players=2,buttons=0x10), type2(coins=2), type0 → **return 0 (OK)**
- with `WAL_TYPEX_JVS_OUTPUTS=20` (decimal; 16 B): `01 02 10 00 | 02 02 00 00 | 12 14 00 00 | 00 00 00 00`
  → type1, type2, **type18(dest[0x24]=0x14)**, type0 → **return 0 (OK)**

`WAL_TYPEX_JVS_OUTPUTS` is parsed as a decimal integer by the current payload. The value `0x14`
would fail parsing and fall back to zero; use `20` to advertise 20 outputs (0x14 in the feature
record).

So feature parsing succeeds either way. 6402 is **not** a parse failure.

---

## What the game actually requires (from the main poll)

Game polls every ~16 ms: `20 02 02 21 02 32 01 00`
- `20 02 02` → SwitchInput: **2 players × 2 bytes**
- `21 02`   → CoinInput: **2 coins**
- `32 01 00`→ GP output: **1 output byte**, value 0x00

Our board's reply (14 B): `01 01 00 00 00 00 01 00 00 00 00 01 12`.

So the board must advertise **at least 1 GP output** in its feature struct.
We set `dest[0x24]` (type 18) for outputs, **but have not confirmed type 18 is
the GP-output field** — if the game checks a different offset (which stays 0),
that would trigger "board doesn't support game spec".

---

## What is NOT the failing check

- **State-5 spec check** (`0x48dbf4`–`0x48dc16`, disasm line ~216248): compares the
  *game's required* spec (`node+0x104` players, `+0x105` buttons, `+0x10c` analog,
  `+0x110` rotary) against **fixed maxes** 2 / 0x18 / 8 / 4. In our run those
  required fields are 0 / in range → **passes**. Failures here log via `0x48d0d0`
  with strings `0x615030/0x615080/0x6150d0/0x615118` ("I/O Board is Unexcepted Spac…")
  — none seen in the log.
- Keychip, platform, DIPSW, file loading, D3D9: all OK.

The failing check must compare the **board-provided** features (`node+0x2a8`)
against the **game-required** spec (the poll: 2P / 2 coin / 1 GP out). That
comparison site is **not yet identified**.

---

## Runtime evidence (latest run, log 15:09)

- Handshake: `F0`×2 → `F1 01` → `11 12 13 14 50` → `10 13` (GetID ×2) → poll loop.
- Identify reply (15:09 build) data (24 B + checksum `AE`):
  `01 01 13 01 30 01 10 | 01 02 10 00 02 02 00 00 12 14 00 00 00 00 00 00`
  (16-B feature block found at idx 8). **NOTE:** this layout does not exactly match
  what the *current* `jvs_board.rs` produces (`01 01 01 13 01 01 01 …`) — the 15:09
  log is from an **earlier build**. Re-run to get a fresh trace.
- `jvs_error.bnk` opened at t≈1673.04, ~90 ms after first poll (t≈1672.60).
- Screenshots are **1024×1536 portrait** (not 1024×768). Earlier note: the game
  creates a 1024×1536 portrait D3D9 device *before* the DIP read — re-check whether
  `DIPSW=BF` (1024×768) is even relevant, or whether this game is genuinely portrait.

---

## Next moves (in order)

1. **Map the feature struct field meanings** (types 3,4,6,7,16–21 → which is GP
   outputs / analog / rotary / lamps). Best sources:
   - The diagnostic dumper `0x411920` (disasm line 27012) + its label strings at
     `0x65bb00..0x65bc50` (`nb_player/nb_button/nb_coin_counter/nb_general_output/
     nb_analog_input/nb_rotary_input`, `ANALOG#%d:%04X`, `DIGITAL[%d]#%d:%02X`).
     Read which `node+0x2a8+off` each label prints.
   - Compare against a **known-good** RingWide game's `14` reply (Let's Go Island
     `lets-go-island-ringwide.yaml` uses `WAL_TYPEX_JVS_OUTPUTS: "20"` and works —
     trace what *it* sends and what that game requires).
2. **Find the spec-comparison site**: search `0x48e410` (binder, line 217025),
   `0x48e4c0`/`0x48e520` (bind loops, 217086/217123), `0x48ed30` (state 3, 217814),
   and the dispatcher `0x48e000` (216635) for a read of `node+0x2a8+<off>` compared
   to the required count. Breakpoint candidates for winedbg: `0x48e933`
   (state-machine fail), `0x582099` (bad record type), `0x48dbf4` (state-5 cmp).
3. **Fresh runtime capture** (the 15:09 log is stale): re-run headless, then
   either (a) winedbg break at the compare and dump `node` = `0x6b023c` (0x6b0 bytes,
   features at `+0x2a8`) + JVS object `0x6b0200` (0x40 B), or (b) add a temporary
   payload dump of that memory right after "1 JVS node(s) found".
4. **Fix** `crates/payload-common/src/jvs_board.rs` `0x14` handler (~line 566) to
   emit the correct record for the GP-output field (and any other fields the game
   requires), keeping valid types only (1–7, 16–21).
5. **Re-test** headless (screenshot must no longer be the error screen), verify with
   `tools/regression/coin-start.txt`, re-check DIPSW / resolution, test
   `TestMode.exe` via `WAL_RINGEDGE_FOREGROUND`, add to `tools/regression/games.yaml`.

---

## Files

- Profile: `systemprofiles/ringedge/pleasant-goat-and-big-big-wolf-happy-happy-playground.yaml`
- JVS board emulation: `crates/payload-common/src/jvs_board.rs` (`0x14` handler ~566)
- Dump: `games/ringwide/Pleasant Goat and Big Big Wolf Happy Happy Playground/`
- Disasm: `/tmp/opencode/hp-disasm.txt`
- Payload log: `wine-prefix/full/drive_c/wal/ringedge/pleasant-goat-and-big-big-wolf-happy-happy-playground/wal-ringedge.log`
- Test layer: `/tmp/opencode/happy-playground-test.yaml`
- Reference (working) RingWide: `systemprofiles/ringedge/lets-go-island-ringwide.yaml`

## Key globals / API (for winedbg)

- JVS lifecycle state `0x6bdd88` (3 = bind-pending); object `0x6b0200`;
  node count `0x6b0238`; nodes `0x6b023c` (stride `0x6b0`).
- Node: `+0x0` spec players (binder gate), `+0x104` req switch players,
  `+0x105` req buttons, `+0x10c` req analog, `+0x110` req rotary,
  `+0x1a0` bound flag, `+0x2a8` **parsed board features (0x34 B)**,
  `+0x2dc..de` rev/jvs/comm, `+0x4ac` board addr, `+0x4ad` node id.
- Feature parser `0x581ef0`; read-all-identify `0x583110`; send-all `0x5830b0`;
  bind binder `0x48e410`; dispatcher `0x48e000`; state entry `0x48e920`.
- Game log fn `0x48d0d0` (`(format, ...)`), hooked via `WAL_RINGEDGE_GAMELOG: 8d0d0`.

## Run command

```
cd /home/aderumier/code/windowsarcadeloader && timeout 150 dist/arcade-launcher run \
  "games/ringwide/Pleasant Goat and Big Big Wolf Happy Happy Playground" \
  --headless --no-input --input-script tools/regression/coin-start.txt \
  --profile /tmp/opencode/happy-playground-test.yaml
```
exit 124 = timeout (game alive = good). BMP shots → `magick shot-NNNN.bmp out.png`.

</details>
