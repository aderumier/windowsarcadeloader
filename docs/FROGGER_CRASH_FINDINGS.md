# Raw Thrills "Frogger" (`sdaemon.exe`) — implementation findings

## Current fixes (2026-10-14)

Status: **works (user: 100%)**, confirmed after gameplay and service-menu verification.

- The post-init crash was a wrong `WaitCommEvent` ABI: Windows takes three arguments.
  The previous four-argument stdcall hook corrupted the 32-bit stack.
- The apparent attract freeze was an infinite USBIO drain loop at `0x24dd00`.
  `ReadFile` always returned another synchronous report, so simulation never advanced.
  The fake now produces bounded status reports and completes pending reads through
  the caller's real OVERLAPPED event, including byte counts and `GetOverlappedResult`.
- USBIO is the watchdog status stream, not the cabinet button stream. Its four bytes
  encode a word through nibble tags `0x10`, `0x20`, `0x30`, `0x40`.
- Buttons come through serial JAMMA `S` reports. Decoder `0x2526f7` consumes nine
  decoded bytes (command plus eight data bytes). The table at `0x2efe58` names the
  modulo-16 counters: `p1s`, `p2s`, `c1`, `c2`, `vlu`, `vld`, `tst`, `srv`.
  Each press and release increments the corresponding counter; serial data bytes
  are encoded as hex pairs between LF + `S` and CR. Object `+4` is configuration,
  so directly writing it does not produce input events.
- Verified live: moving animal rows, both players receiving credits and completing
  rounds with ticket scores, and volume changes. Holding Test enters the service
  menu; Volume Down changes the selection. Escape exits through the launcher again.

The following investigation is historical; its speculative causes and input mapping
were superseded by the fixes above.

Findings from reverse-engineering the launcher + hardware fakes for this cabinet.
Written to hand off to another engineer/AI. **Confirmed facts** are stated plainly;
**hypotheses** are labeled and ranked. All analysis is from a clean 32-bit PE read
(`.protect` is only encrypted at rest around the dongle check — the rest of the binary is
plain and fully statically analyzable).

---

## 1. Goal

Make Raw Thrills / ICE 2011 **"Frogger"** (the "S" engine, `sdaemon.exe`, a 2-player
redemption cabinet) launch through the existing **Windows Arcade Loader** and be playable
from the virtual sticks. Original complaint: **"I/O error. Input not working."**

Approach: **fake all cabinet hardware + the copy-protection dongle with pure IAT hooks
and sentinels. No code patching.** The game must come up to the attract / coin state so
that start / test / service / coin from the virtual buttons work.

## 2. Launch path & addresses

| Item | Value |
|------|-------|
| Launcher command | `wal-loader.exe wal_rawthrills.dll sdaemon.exe` |
| Profile | `systemprofiles/rawthrills/frogger.yaml` (`exe_fixed_base: true` — the game has **zero relocations**) |
| Payload module | `crates/payload-rawthrills`, game-specific code in `src/frogger.rs` |
| Dispatch env | `WAL_RAWTHRILLS_GAME=frogger` |
| Game image base | **`0x230000`** (fixed). Image spans `0x230000..0x596000`. |
| Payload load base | `0x7af70000` (per run) |
| Run under | GE-Proton 11.7 (Wine), 32-bit WOW64 process |

### VMA → file mapping (ImageBase `0x230000`)

| Section | runtime VMA range | file offset = |
|---------|-------------------|---------------|
| `.text` | `0x231000 .. 0x2d4000` | `0x400 + (vma − 0x231000)` |
| `.rdata`| `0x2d5000 .. 0x2db000` | `0xa3400 + (vma − 0x2d5000)` |
| `.data` | `0x2db000 .. 0x3ea000` | `0xa9400 + (vma − 0x2db000)` |

So a runtime address in `.text` (e.g. the reader at `0x2540a0`) maps to file
`0x400 + (0x2540a0 − 0x231000)`. Anything ≥ `0x3ea000` and < `0x596000` is `.rdata`/`.SCY`.
A value like `0x00360034` = base + RVA `0x130034`, which is **in `.data`** (a data pointer,
**not** a code return address).

## 3. Hardware the game opens (all faked)

1. **USBIO board** (buttons / coins / lamps). Found via SetupAPI (class GUID
   `96df5d32-8c93-d311-9e34-0080c82727f4`), opened with `CreateFileA`
   (`FILE_FLAG_OVERLAPPED`), driven with `DeviceIoControl`. Opened **3×** (main, input
   pipe, output pipe). Input pipe is read as 4-byte frames by the game's I/O thread.
   - version read `0x80942006` → 18-byte blob, words `0xc70`/`0x774` at offsets 8/10
   - init `0x80942024`, reinit `0x80942054`
   - capability `0x8094206c` → 12-byte reply, leading word `0x141`
   - pipe role `0x80942078` → 1 byte (`0x81` input, `2` output)
2. **Serial JAMMA board** — probed on `COM1..COM16` (function `0x253aa0`), **found on
   `COM2`**. This is the board that carries switch/coin/start/test/service.
3. **LED bonus display board** (`0x24f730`) — 6-byte frames, on `COM1`. Faked: opens,
   logs frames, never answers reads (harmless — the LED init fails through and stays
   inactive).
4. **HASP dongle** — `LoadLibraryA("API_1LNM.DLL")` + `GetProcAddress(...,"HLW32Proc")`,
   called from inside the encrypted `.protect`. Faked: fake module + a stub that logs the
   command and returns success.

## 4. What already works (verified in runs)

- All fakes install cleanly; USBIO is detected; the HASP stub is called.
- JAMMA probe: `COM1` scan **rejected**, `COM2` **accepted** — the probe writes the
  `C`/`U`/`v` frames and `COM2` answers `v` with the 7-byte version reply
  `[0a 76 31 2e 30 31 0d]`.
- `jamma_open` (`0x253dd0`) **succeeds** (open #2): handle stored, `SetCommMask(1)`,
  reader (`0x2540a0`) and writer (`0x2537d0`) threads launched, initial **73-byte burst**
  sent, then a 4-byte `\ns\r\0`.
- Coin / start / test / service / volume are wired from the virtual sticks into the USBIO
  input pipe (coin = `B==0xc0 & A&1`; idle frame `1c 20 34 40`, coin `1c 20 34 41`).

**The game then wild-jumps and dies ~0.3 s later. This is the crash to fix.**

## 5. The crash (deterministic)

- ~0.3 s after `JammaInit` succeeds, the process **jumps to a non-code heap address**.
- Exit status **5** (Wine "unhandled page fault on execute access"), **no `ExitProcess`**.
- **`Eip` = `0x1BDFF40`** — a pointer into a heap block near the top of a 64 KB page.
  **The target varies per run** (`0x1BDFF40`, `0x1CDFF08`, …) ⇒ a **runtime heap pointer**,
  not a constant.
- Registers at the fault (identical across multiple runs; from the **authoritative**
  `crash.rs` reporter, which uses the real `windows-sys` `CONTEXT` struct):

  ```
  EAX=00000000  EBX=00000001  ECX=7be2f818  EDX=00000006
  ESI=00000103  EDI=00000000  EBP=00371668  ESP=001bdef00
  ```

- **`EBP` = `0x371668` = the JAMMA board object** (a static global in `.data`).
- **The stack at and around `ESP` is all zeros.** The `EBP` frame chain is **broken**
  (`EBP` holds the object, not a stack frame). No valid backtrace can be recovered from
  the stack.
- It happens on the **JAMMA reader thread** — the thread doing the
  `WaitCommEvent → PurgeComm` idle spin (confirmed by thread id in the log).

> Note: a custom vectored-exception-handler (VEH) was added in `frogger.rs`
> (`hook_crash`/`crash_handler`). Its raw `CONTEXT` field offsets were **buggy** (and one
> `EXCEPTION_RECORD` field was mislabeled), so **do not trust the VEH's register printout**.
> Trust `payload-common/src/crash.rs` (the pre-existing reporter) — its register dump is
> correct and is the source of the values above.

## 6. Analysis — what the crash is

- It is a **wild jump**: control transferred to a data/heap address and faulted there.
- **It is not a register jump** — no general-purpose register holds the target value at
  fault time. So the transfer was either:
  - **`ret` to a corrupted return address** (a local buffer overflow smashed the frame), or
  - **`call/jmp [mem]`** where `[mem]` holds a heap pointer (a corrupted function pointer /
    vtable / callback slot).
- The faulting function holds the JAMMA object in **`EBP`**. The reader loop itself
  (`0x2540a0`) keeps the object in **`EDI`** (`mov edi,[ebp+8]`), so **the crash is in a
  *different* function that the reader calls, one that uses `EBP` for the object.**
- The JAMMA object's watched slots — `+0x00, +0x04, +0x08, +0x10, +0x14, +0x18,
  +0x1b0, +0x1b4, +0x208, +0x20c` — **never changed once** during the run (the watcher
  polled every 16 ms). In particular the **event-callback slot `+0x18` never became a
  pointer.** So the wild pointer is not any of those 10 slots (only 10 slots were watched;
  other slots could still hold a pointer).
- The crash target changes per run ⇒ heap pointer, not a code constant.
- The stack scan found **my payload code referenced** (`[esp+0x2b4] = wal_rawthrills+0x69e88`),
  consistent with the reader thread calling my IAT hooks (`WaitCommEvent`/`ReadFile`/…).
  The exact function is **not** resolvable (Rust symbols are stripped from the `.dll`).

## 7. Ruled out

- **Not** the `call [esi+0x18]` at `0x252c7f` (the only indirect member call in the JAMMA
  region): that code has the object in `ESI`, but the crash has it in `EBP`; and `+0x18`
  never became a pointer.
- **Not** a `.text` jump table (those are code constants, deterministic).
- **Not** a direct call into the reader loop (the object would be in `EDI`).
- **Not** the `GetCommState` DCB overrun (fixed by clamping the zeroed DCB length to
  `8..=0x1c`) and **not** the `jamma_on_write` off-by-one (both fixed in earlier
  iterations; the crash signature is unchanged by those fixes).

## 8. Reversed game structures

### JAMMA object — static at `0x371668`
The compiler hardcodes its address (e.g. `cmp BYTE PTR ds:0x371668,0` at `0x254113`).

| Off | Meaning |
|-----|---------|
| `+0x00` | open flag (reader exits if it is `0` and the handle is `0`) |
| `+0x04` | switch word: bit `0x40` = checksum on; `0x2000/0x4000/0x8000` = inputs; `0x20000/0x40000/0x80000` = dispatcher flags |
| `+0x08` | error flags (reader sets `1/2/4/8`; purges the comm on any) |
| `+0x10` | config (bit 0 → 9600 baud, else 115200) |
| `+0x14` | tx flush size |
| `+0x18` | **event-callback pointer** |
| `+0xa0..` | player data area |
| `+0xcc` | lamp-rate tick |
| `+0xd0..0xdc` | lamp timers |
| `+0x1b0` | comm handle |
| `+0x1b4`/`+0x1b8` | reader / writer thread handles |
| `+0x1bc` | port index |
| `+0x1c4..0x1f0` | rx/tx ring (changes every tick — excluded from the watcher) |
| `+0x208`/`+0x20c` | mutexes |

Related globals: `0x374f78` JAMMA active, `0x374f7c` checksum-fail count, `0x37182c` rx
buffer (0x1000), `0x371878`/`0x371a98` callback args, `0x2f98d0` log context, `0x374f6c`
LED handle, `0x374f70` LED active, `0x374f8c` reader loop counter, `0x2dbd0c` `/GS` cookie.

### Reader thread — `0x2540a0`
- Prologue `push ebp; mov ebp,esp; mov eax,0x1060; call 0x2d3ab0` → **4192-byte frame**
  (`0x2d3ab0` is the MSVC stack probe, verified safe).
- Object in **`EDI`** (`mov edi,[ebp+8]`).
- `CreateEventA(0,0,0,0)` → `[ebp-0x1050]`.
- Exit if event is `INVALID_HANDLE`, or `[obj+0x1b0]==0`, or `[0x371668]==0`.
- Else `SetCommMask(h,1)`, loop at `0x254152`:
  - `call 0x2509d0` (a "keep going?" predicate; `al==0` → exit at `0x25455e`)
  - `[0x374f8c]=0`; `Sleep(1)`
  - if `[obj+8]`: `PurgeComm(h,0xf)`; `[obj+8]=0`
  - `WaitCommEvent(h, &mask[ebp-0x104c], &ov[ebp-0x1060])` — **only 3 args pushed**; the 4th
    (byte-count) is stale stack garbage (so the fake must guard that slot with an
    `is_ptr` check).
  - `ret!=0` → if `mask & 1`: `ReadFile(h, buf[ebp-0x1024], 0x1000, …)`
  - `ret==0` → `GetLastError`: `PENDING` → `GetOverlappedResult` (fail → `[obj+8]|=2`);
    other error → `[obj+8]|=1`; loop.

### `jamma_open` — `0x253dd0`
- Per-port probe (`0x253aa0`) with `CreateFile` (retried ≤10×). Probe frames:
  `C = \nC0001\r1B`, `U = \nU00\rCC`, `v = \nv\r8D`. Poll loop 10× {`WriteFile(v)`;
  `Sleep(10)`; `CreateEventA` (ignored); `ReadFile(buf, 0x100)`}. Accept iff 7 bytes
  `0x0a / 0x76 / … / 0x0d` → 0-based port (`COM2` = 1).
- Then: `CreateFileA(COM2)` → `[obj+0x1b0]`; `SetCommTimeouts {0xffffffff,0,0,0,0}`;
  reader `0x2540a0` → `[obj+0x1b4]`; writer `0x2537d0` → `[obj+0x1b8]`; mutexes
  `[obj+0x208]/[0x20c]`; `[obj+0]=1`.

### Serial protocol
- Frame = `\n` + command + data + `\r` + **two uppercase-hex checksum chars** =
  `sum(bytes \n..\r inclusive) & 0xff`.
- **Post-init frames are sent WITHOUT the checksum** (bit `0x40` of `[obj+4]` is not set).
  The parser must handle both modes: if the two bytes after `\r` are not both uppercase
  hex, then `\r` is the terminator. **This no-checksum mode is NOT implemented in the
  fake yet (see §10).**
- Response builder `0x250e10`: jump table `0x25139c`, index = `ecx−1`; appends
  `\r` + checksum at the tail, gated on `[obj+4] & 0x40`.

### Key IAT slots (kernel32 block, in the game)
`WriteFile 0x2d50e0 · ReadFile 0x2d50c0 · CreateFileA 0x2d5088 · GetCommState 0x2d50ec ·
SetCommState 0x2d50dc · SetupComm 0x2d509c · PurgeComm 0x2d5058 · GetCommTimeouts
0x2d50b8 · SetCommTimeouts 0x2d50c8 · ClearCommError 0x2d5084 · WaitCommEvent 0x2d5050 ·
SetCommMask 0x2d504c · CreateEventA 0x2d5090 · GetLastError 0x2d50bc · Sleep 0x2d50f4 ·
GetOverlappedResult 0x2d5074`. Also `0x2d5234` msvcr `memset`; `0x2d5278/0x2d5240/0x2d52a4`
log-fn format helpers; `0x2513f0` JAMMA log helper; `0x2509d0` reader keep-going check.

## 9. What the current fake does (post-init)

- Handles: `COM1 → SER_LED (0xf5b0005)`, `COM2 → SER_JAMMA (0xf5b0006)`,
  `COM3..16 → INVALID` + `err 2`. USBIO: `0xf5b0001/2/3`, path `\\.\WALUSBIO0`.
- JAMMA fake: **answers `v` only while `JAMMA_OPENED == 1` (the probe).**
  **After open #2 it sends nothing.**
- Comm hooks (fake handles → success, else fall through to the original):
  - `GetCommState` — zero the DCB, **clamped** to `8..=0x1c` bytes (the game's DCB local
    is exactly `0x1c`, not the full `0x34` struct)
  - `SetCommState`, `SetupComm`, `SetCommTimeouts`, `SetCommMask` — no-op success
  - `PurgeComm` — **no-op** (clearing the queue here would drop queued replies)
  - `GetCommTimeouts` — zero `0x14` bytes
  - `ClearCommError` — 0 errors, `cbInQue` = pending length
  - `WaitCommEvent` — if JAMMA RX non-empty → `*mask=1`, return 1 (and write the byte
    count only if the slot `is_ptr`); else return 0 + `ERROR_IO_PENDING`
  - `GetOverlappedResult` — `Sleep(1)` + 0 + `ERROR_IO_PENDING`
- Reader's post-init spin against the fake: `WCE → PENDING → GetOverlappedResult fail →
  [obj+8]|=2 → next iteration PurgeComm(0xf) → …` (~every 2 ms). Data queued in `JAMMA_RX`
  is picked up when `WCE` sees it non-empty.

**The fake sends no board→game responses after open #2, and it does not even parse the
no-checksum post-init frames (see §10).** So from the game's point of view, its initial
burst + `\ns\r\0` get **no acknowledgement**.

## 10. Hypotheses, ranked (what to try next)

### (A) Most likely — missing post-init board responses + no-checksum parse
After open #2 the game sends an initial 73-byte burst and a `\ns\r\0` frame and expects
the real JAMMA board to **respond** (acks to `C0081`, a reply to the second `v`, etc.).
The fake currently:
1. sends **nothing** after open #2, and
2. **does not parse** the no-checksum frames (its parser `jamma_frame` requires a valid
   2-char uppercase-hex checksum; a no-checksum frame like `\ns\r\0` never satisfies
   `tx.len() >= rl+3`, so it is never dispatched — it just sits in the buffer).

If the reader/dispatcher waits for (or time-mis-handles) the missing response, it can
read out of bounds or corrupt a pointer → the wild jump. **This is the first thing to
test.**
- **Do:** implement the **no-checksum parse mode** (`\r` is the terminator when the next
  2 bytes are not both uppercase hex), and start sending the **post-init board→game
  responses** (ack to `C0081`, reply to `v`, whatever the logged traffic implies the board
  sends). Iterate against the log.

### (B) Stack buffer overflow in a reader-callee that holds the object in `EBP`
The reader's data path `ReadFile(h, buf[ebp-0x1024], 0x1000, …)` then dispatches the
received frame to a handler. A handler that (a) takes the object in `EBP` and (b) has a
local buffer that can overflow would smash its own return address and `ret` into a heap
pointer. **Find function `F`:** disassemble the reader region (`0x2540a0..0x254600`) and
its callees; look for `mov ebp, ds:0x371668` (or `mov ebp, […object…]`) followed by a large
frame and a `ret`, or a `call [reg+disp]` into an object slot.

### (C) A corrupted object slot we are not watching
Only 10 slots are watched. If a *different* slot holds a callback/pointer that the game
populates from a board response we never send, it would be garbage/heap. Watch **all**
non-ring slots of the object and see which one turns into a heap pointer before the crash.

## 11. Unresolved questions

- Does the crash require the **absence** of board responses? (test by adding them)
- Which function holds the object in `EBP` and performs the wild jump?
- Is a specific object slot (other than the 10 watched) written with a heap pointer?
- Is the **no-checksum** post-init mode load-bearing for the dispatcher? (The current
  fake neither parses nor answers those frames.)

## 12. Reproduce

```bash
cd /home/aderumier/code/windowsarcadeloader
./build.sh                                   # → dist/arcade-launcher + dist/payloads/wal_rawthrills.dll
# (in a separate shell, if needed:)
rm -f wine-prefix/full/drive_c/wal/rawthrills/frogger/wal-rawthrills.log
dist/arcade-launcher run games/rawthrills/Frogger   # run ~25 s, then inspect the log
```

- Payload log: `wine-prefix/full/drive_c/wal/rawthrills/frogger/wal-rawthrills.log`
  (flushed per line, overwritten each run).
- Authoritative crash dump: the `crash: …` lines (from
  `crates/payload-common/src/crash.rs`), not the `frogger: CRASH` VEH lines.
- 32-bit debugger if needed:
  `wine-runners/GE-Proton11-7-x86_64/lib/wine/i386-windows/winedbg.exe`.

## 13. Source files

- `crates/payload-rawthrills/src/frogger.rs` — the payload: all fakes, the IAT hooks, the
  (buggy) VEH crash catcher, the JAMMA object watcher, the input thread.
- `crates/payload-rawthrills/src/lib.rs` — `DllMain` dispatch + reboot hooks.
- `crates/payload-common/src/crash.rs` — **authoritative** VEH crash reporter
  (register dump, EBP walk, symbolized stack scan, `heartbeat`/`dump_thread` hang
  reporter — reusable for suspended-thread dumps).
- `crates/payload-common/src/iat.rs` — bound-IAT slot matching (the `hook()` helper).
- `crates/launcher/src/main.rs` (~line 393: `wal-loader <payload> <game>`, `--dry-run`
  prints env + cmdline) and `crates/launcher/src/wine.rs`.
- `systemprofiles/rawthrills/frogger.yaml` — the profile (`exe_fixed_base: true`).
- `games/rawthrills/Frogger/sdaemon.exe` — the PE.
- Disassembly references: full Intel disasm at base `0x230000` (note a misaligned region
  `0x2522e9..0x2523f7`); the IAT slot→function map (kernel32 block at `0x2d50xx`; the
  `0x2d3xxx` addresses are **game** functions/thunks, not IAT).

## 14. Uncommitted work (as of writing)

- Modified: `crates/launcher/src/systems/rawthrills.rs`, `crates/payload-rawthrills/src/lib.rs`
- New: `crates/payload-rawthrills/src/frogger.rs`, `systemprofiles/rawthrills/frogger.yaml`
- Update when working: `docs/IMPLEMENTATION.md`, `tools/gamelist.py`.
