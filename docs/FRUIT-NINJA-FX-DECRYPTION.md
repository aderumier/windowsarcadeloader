# Fruit Ninja FX: recovering the executable without the Matrix dongle

## Result and scope

Recovered the original Fruit Ninja FX executable from its Matrix envelope using REA MCP
and controlled x86 emulation. No dongle, dongle recording, second executable, or cabinet
memory dump was available. The encrypted original was preserved.

The protected game sections use XTEA encryption followed by aPLib-style decompression.
For **this build**, the section key can be derived from a table in the loader. Dongle
authentication prevents reaching that code normally, but the optional dongle transformation
of the generated key is disabled. Skipping authentication alone was insufficient: the
loader also applies compensating adjustments to the first key byte.

The rebuilt executable starts under Wine and initializes a 1920×1080 Direct3D 9 display.
Full gameplay, cabinet I/O, coins, tickets, touch input, and long-term execution have not
been validated. Decryption and cabinet emulation are separate tasks.

| Item | Value |
| --- | --- |
| Input | `games/adrenaline/Fruit Ninja FX/FruitNinja.exe` |
| Input size | 645,632 bytes |
| Input SHA-256 | `6c8254348af42e7ccc24719b0d1eecc0856a6d7207d391be4144c597c93fc460` |
| Output | `games/adrenaline/Fruit Ninja FX/FruitNinja.decrypted.exe` |
| Output SHA-256 | `82de7eb32d7fdb6e87bbaee8100aca77d532d9f60604d53dcca248844ab2b418` |
| Architecture / preferred image base | x86 PE32 / `0x400000` |
| Protected entry RVA | `0x1de169` |
| Original entry RVA / VA | `0x651db` / `0x4651db` |
| Recovered XTEA key, bytes in memory order | `d16571b43c9720a978297466ffe3a73a` |
| Original import table RVA | `0x14e17c` |
| Recovered imports | 12 DLLs, 187 functions |

## Reproduce this exact recovery

The retained script uses only the Python standard library. It verifies the input hash,
decrypts and decompresses the protected sections, reconstructs the PE layout and imports,
and computes the original entry point. It does not require REA, Wine, Unicorn, or a dongle
to reproduce the final executable.

```bash
python3 tools/scripts/fruit-ninja-fx-decrypt.py \
  'games/adrenaline/Fruit Ninja FX/FruitNinja.exe' \
  'games/adrenaline/Fruit Ninja FX/FruitNinja.decrypted.exe'
```

The input and output must differ. The script rejects other input hashes. Its hard-coded
key, offsets, PE assumptions, and entry encoding belong to this exact executable; port
the investigation, not these constants, to another game.

## 1. Identify the protection before assuming the key is lost

Inventory the supplied dump and hash its executable. Wait for any copy/extraction to finish
before treating the inventory as complete: files were still arriving during this session.

REA's `inspect_pe_imports` found only `LoadLibraryA` and `GetProcAddress` in the protected
executable's import table. Its section names were blank, and the entry point was in the
last section. These observations support inspecting a loader; they do not prove a
particular cipher or that the key resides in external hardware.

Open the executable with `open_binary` and inspect the entry using `procedure_assembly`
and `procedure_pseudo_code`. Hopper was the available deep provider in this session.
Use raw instructions to resolve self-modifying code, stack tricks, and suspicious
decompiler arguments. Pseudocode alone misrepresented several of them here.

Record original file offsets separately from RVAs and VAs. In this case, for example,
the last section starts at RVA `0x1de000`, VA `0x5de000`, file offset `592384` (`0x90a00`).

## 2. Unwrap the software loader first

The entry at `0x5de169` uses two call/pop sequences to obtain its own address and the
constant `5`. It then enters a chain of self-modifying routines. Useful locations were:

| VA | Role |
| --- | --- |
| `0x5de098` | Outer bootstrap |
| `0x5de153` | Saves position/base information and redirects to a later layer |
| `0x5ead7d` | Prepares a call operand and another layer |
| `0x5eae33` | Reverse rolling-XOR decoder |
| `0x5ead9c` | First recovered outer-layer body |
| `0x5de0c8` | Bytewise XOR loop with immediate `0xa2` |
| `0x5de17e` | Main loader body after outer decoding |

For the first rolling-XOR layer, the key byte is the return instruction at `0x5ead9b`
(`0xc3`), the decoded range starts at `0x5ead9c`, and its exclusive end is `0x5eae33`.
Iterate backward; before XORing each byte, XOR the running key with that byte's index
relative to the decoded range, reduced to eight bits. An initial attempt starting one
byte late produced invalid code. Stack-derived boundaries matter.

Rather than manually reimplement every outer layer, map the PE image into Unicorn at
its preferred base, allocate a stack, install explicit synthetic API stubs, and execute
the real x86 loader. Keep bounded instruction counts and stop on unknown APIs. Save
snapshots at useful boundaries and open those derived files in REA for further analysis.

The simulation implemented library/procedure lookup, basic allocation, version/time
queries, and no-device responses for SetupAPI/HID. The default no-device run reached
the message `Dongle not found`. That result describes the synthetic environment; it
does not establish that the encrypted data cannot be recovered offline.

Useful checkpoints were the decoded outer loader and the image immediately before
authentication at `0x5e2245`. A second checkpoint at `0x5e0b32`, before import restoration,
preserved all decompressed sections and their original import names.

## 3. Trace the key's producers and consumers

Follow both sides of the key buffer, instead of treating the authentication return value
as equivalent to the decryption key.

| VA | Role |
| --- | --- |
| `0x5e10eb` | Authentication wrapper and first-byte adjustment |
| `0x5e2245` | Authentication logic, key generation, optional dongle transformations |
| `0x5e19d3` | Lookup-table key generator |
| `0x5e78d9` | Key-generation table |
| `0x5e7959` | Configured user code: `0xbc9c` |
| `0x5e70e5` | Enables optional dongle cryptographic transformations; zero in this build |
| `0x5e70f9` | 16-byte section-decryption key buffer |
| `0x5e1de3` / `0x5e1ecf` | Dongle encrypt / decrypt dispatch, commands `0x45` / `0x44` |
| `0x5e00e1` | Section processing |
| `0x5e0d3e` | Applies block decryption over a range |
| `0x5e0cc4` | XTEA block-decryption primitive |

The key generator processes eight nibbles of the user code from least significant to
most significant. For each nibble it selects two bytes from the table. A zero nibble
uses a separate incrementing counter. In pseudocode:

```python
zero_counter = 0
key = bytearray()
for row in range(0, 64, 8):
    column = user_code & 15
    if column == 0:
        column = zero_counter
        zero_counter = (zero_counter + 1) % 16
    index = 2 * (row + column)
    key += table[index:index + 2]
    user_code >>= 4
```

With this executable's decoded table and user code `0xbc9c`, the result is
`d16571b43c9720a978297466ffe3a73a`. The key generator also writes a terminating zero
after the 16-byte buffer, which matters to the later entry-point checksum.

Authentication may optionally transform the two eight-byte key halves through the
dongle. The switch at `0x5e70e5` is zero here, and the inspected references were reads.
This is the important build-specific observation that made offline recovery practical.
Another Matrix-protected game may enable that path and genuinely need a device-derived
result.

### Why the first bypass failed

The first emulation bypass returned success from the authentication routine and installed
the generated key directly. After returning, the surrounding wrapper changed its first
byte from `0xd1` to `0x2f`, producing invalid compressed data.

The real authentication path also adjusts that byte internally. Omitting the internal
adjustment while retaining the external one changes the effective key. In the corrected
experiment, the simulated internal adjustment compensated the wrapper, and the section
decryptor received the original generated key. The final offline script bypasses the
loader entirely and uses that verified effective key.

We tested all 256 possible first-byte values for the generated key, plus two alternate
user-code hypotheses, against a bounded prefix of the first compressed section. The
only surviving candidate consumed the entire 512-byte prefix and produced 956 bytes
of plausible x86 code. This was candidate selection, not the final proof: full-stream
decompression and independent byte comparisons supplied the stronger validation below.

## 4. Decrypt, then decompress

The block primitive is little-endian XTEA: eight-byte blocks, a 16-byte key, 32 rounds,
initial decryption sum `0xc6ef3720`, delta `0x9e3779b9`, with 32-bit wrapping arithmetic.

The PE timestamp `0x3f` is repurposed by the envelope as two flags per section:

- Bit `2*i`: decompress section `i`.
- Bit `2*i+1`: decrypt section `i` before decompression.

Thus the first three sections have both flags set. The fourth contains resources, and
the fifth is the protection loader.

The loader trims trailing zero padding to determine the encrypted range, then decrypts
only complete eight-byte blocks in that range. **Keep the remaining raw bytes and zero
padding available to the decompressor.** Two compressed streams need their final zero
terminator beyond the trimmed range. Removing that padding caused an apparent incomplete
decompression despite having the correct key.

The decompressor at `0x5e05b8` uses the literal, variable-length offset, previous-offset,
short-match, and four-bit single-byte forms of an aPLib-style stream. Translate its
instructions or execute that routine directly. Validate reference distances and bound
the output by the section's virtual capacity when testing candidate keys.

| Section RVA | Trimmed encrypted bytes | Decoded bytes | Virtual capacity |
| --- | ---: | ---: | ---: |
| `0x1000` | 504,512 | 1,122,808 | 1,126,400 |
| `0x114000` | 78,531 | 242,252 | 245,760 |
| `0x150000` | 6,954 | 35,581 | 577,536 |

The large zero-filled remainder of the third section is expected writable storage.
Successful decompression need not fill the entire virtual section.

All three statically recovered sections, including their zero-filled remainders,
matched the independently emulated loader output byte for byte.

## 5. Recover the original entry point

Do not mistake `0x27d4` at `0x5e70bd` for the original entry RVA. It selects an exception
handler relative to the loader's own position. An initial provisional entry using that
value landed in the middle of an ordinary game function.

The final transfer intentionally faults at `0x5e0013`. Handler `0x5e093d` rewrites the
x86 context, and `0x5e03e0` finishes decoding the target. Mapping a readable page at
address zero for the emulator's synthetic FS state suppressed the fault and made this
transfer fail. That was an emulation limitation, not another encryption failure.

REA decompilation plus the handler's instructions established this build's transform:

```python
mask = 0xffffffff
magic = 0xc3c3c3c3 ^ 0x11091973       # 0xd2cadab0
intermediate = ((0xff596e36 + magic + 1) & mask) ^ magic
checksum = 0
for index, byte in enumerate(key):
    checksum = ((checksum + byte * index) * 2) & mask
entry_rva = intermediate ^ checksum  # 0x651db
```

The checksum routine is at `0x5e0742`; it stops at the key's terminating zero.
The recovered entry at VA `0x4651db` calls CRT/security initialization and enters
the normal Windows program startup. REA decompilation confirmed that code path.

## 6. Reconstruct the PE

Capture imports **before** the loader resolves them: its restoration routine erases
descriptors and name data after resolving functions. A later dump with synthetic API
addresses is unsuitable as an on-disk executable without rebuilding those imports.

For this image:

1. Keep the first four original virtual sections and their RVAs.
2. Write the decoded bytes and zero-fill the remaining virtual capacity.
3. Give each section a raw pointer equal to its RVA and a raw size equal to its virtual
   capacity. These values satisfy the input's alignment constraints.
4. Use `.text`, `.rdata`, `.data`, `.rsrc` with conventional permissions. These names are
   reconstructed labels, not recovered original names.
5. Remove the fifth loader section and its header; set `SizeOfImage` to `0x1de000`.
6. Restore the entry RVA `0x651db`.
7. Restore the import directory at RVA `0x14e17c`, size 260 bytes: 12 descriptors and
   their terminating empty descriptor. Preserve named and ordinal imports.
8. Clear the repurposed timestamp, stale IAT directory, and PE checksum.
9. Preserve the remaining relevant original directories, including resources.

The import set includes `d3d9.dll`, `d3dx9_42.dll`, `DINPUT8.dll`, `WINMM.dll`, `WS2_32.dll`,
`PQMTClient.dll`, `KERNEL32.dll`, `USER32.dll`, `ADVAPI32.dll`, `SHELL32.dll`, `ole32.dll`,
and `OLEAUT32.dll`.

## 7. Verify the result in stages

Verification completed here:

- Original input SHA-256 still matches.
- All three decompressed sections match the emulated loader byte for byte.
- All 12 import DLLs and 187 function imports parse, including ordinal imports.
- REA opens the final rebuilt image and decompiles the recovered CRT entry point.
- A bounded launch of the final image under GE-Proton11-10 in the existing `fn-test`
  prefix reaches Direct3D 9 initialization and creates a 1920×1080 display. The run
  is stopped by a timeout; this does not establish working gameplay.

Do not describe a candidate key, plausible disassembly, or an import inventory as proof
that the complete game works. Keep decryption validation, startup validation, and actual
cabinet/gameplay validation separate.

## Applying the method to another game

Start with its own hash and protection layout. Reuse these questions:

1. Is the game code encrypted, compressed, or both? Which bytes are only loader code?
2. Does the dongle only gate execution, produce a cryptographic key, or transform data?
3. Where is the effective section key consumed, and which writes produce it?
4. Are there internal/external byte adjustments that cancel only on a successful path?
5. Are optional hardware cryptographic branches enabled in this particular build?
6. Can candidates be rejected using bounded decompression and valid backreferences?
7. Does the original entry use SEH or context rewriting instead of a direct jump?
8. Does import restoration destroy the metadata needed for a disk rebuild?
9. Do independently recovered bytes match before claiming successful decryption?

If the effective key depends on a secret held only by the missing dongle and cannot be
derived from the supplied artifacts, this example provides no general way around that
cryptographic dependency. A common protection family does not imply a common key or
the same optional settings.

## Evidence and retained artifacts

`REA-analysis/Fruit Ninja FX/` at the repository root contains the outer-loader snapshots, the original
emulation harness, logs, and individually retained REA records. Historical failed
hypothesis logs are retained as such. The final dependency-free script is versionable
in `tools/scripts/fruit-ninja-fx-decrypt.py`; game binaries remain with the local dump.

Key REA evidence records:

| Evidence | What it supports |
| --- | --- |
| `ev_b87d091a9385f3806552a965fb03fab094210df5ecbc0f6c8ee6ef366977849f` | Protected import table |
| `ev_f5eb96bcb3ea479c68c11dec8e683004a0eb56c7dd107177f05bc618a895025c` | XTEA block-decryption primitive |
| `ev_da6d3ec7db403ad648d8417e25d667609c0f0c7896583bace4d9479752ff062d` | Lookup-table key generation |
| `ev_c91787ab5647b6dc97c8f94dfd2e27952abcde76384995dc676f72d611f49fce` | Authentication/key control flow |
| `ev_c31c078d105be66e5b7014750228f382eec48f3167ff036a01851d11c0faeb16` | Decompression routine |
| `ev_ab1fdd26ad1fe3353dda37b843de20bd20e33ed3c6aebe3f878766c2a2b68fe1` | Exception-based entry-point transfer |
| `ev_6b352bc76018a621c5ea324dd8998bf978ec5d877bbd214b02db1f6adcbc2147` | Final entry-point decoding path |
| `ev_fc907ae86dc6d20c4968baed5915b951ba3c8dcfe74320e7670fa33d41958490` | Final executable identity and recovered startup code |

REA static pseudocode supports code structure. The byte comparison and Wine startup
observations came from the local verification commands, not from a REA runtime capture.
