# Far Cry Paradise Lost: in-level debug notes

Status (2026-10-30): **levels play** (crates, barrels, health boxes, moving and shooting
enemies, first person camera on the mounted guns). Root cause below: Wine's builtin
`msvcr71` clamps out-of-range numbers where Microsoft's wraps them; the profile uses the
game's own `msvcr71.dll` / `msvcp71.dll` (`dll_overrides` `n,b`).

Symptoms before the fix: after the intro fly-over the camera sat at sea level near the pier,
enemies stood still like statues, the trigger fired nothing ("SHOTS FIRED 0"), the health went
down as if drowning; no crates, barrels or health box on the beach, even during the fly-over.
Same with another launcher under Wine; the dump played on Windows.

## Root cause: the dependency CRCs (2026-10-30)

- At level load `0x4583b0` reads the level's dependency list (`<level>_DepLoad.xml` in the
  level `.dat`: `<Entity Class Name CRC>` with `<File CRC Type Name>` children) into a map
  keyed by entity CRC (resource manager `0x9d44ac` + 0x2c). The CRCs are written in decimal.
- Hash `0x783c80` = CRC32; names go through `0x4653d0` (lower case, `.glm` -> `.xbg`, `/` ->
  `\`, leading `\`): entity key = CRC32 of `\` + lower-case name, file key = CRC32 of `\` +
  lower-case path.
- An entity asks for its files when it starts loading (`0x491680` -> `0x455640`: list from
  `0x457f20`, each file loaded into the resource table), then waits (`0x491440`) until each
  object slot's model has its data (`0x7bf8f0`: resource lookup `0x458010`; not found and not
  failed (`0x458140`) = wait). Only then is the box computed, the pending changes applied and
  the entity registered for drawing.
- With Wine's builtin `msvcr71`, every CRC from 0x80000000 up is read as 0x7fffffff (clamped),
  so about half the files (crate `e4b698a1`, barrel `e10483d8`, health box `ba5bc4aa`, chair
  `a662571a`, character animations...) are never loaded: those entities wait forever (no
  radius, never drawn, never physicalized, never updated). The player's "Cam First" bone
  animation is in that half too (hence the camera at the feet). Microsoft's `msvcr71` wraps the
  value and keeps the right 32 bits.
- Fix: `dll_overrides: msvcr71: "n,b", msvcp71: "n,b"` (both DLLs are in the dump).

The sections below are the investigation that led there (the `mounted_eye` payload
workaround described in it was removed once the real cause was found).

The dump is now built from the game's install disc (`Paradise Lost.iso`, InstallShield
`data1.cab`, "Game" component): every file is the same as in the older dump except
`FarCry_r.exe`, which was a no-dongle crack there (7 bytes: the dongle checks and the control
error screens). The disc has no `systemcfg.lua`: the profile installs one
(`g_arcadeNeedCalibration = "0"`), the game completes it.

## Done

- **Dongle** (`payload-globalvr/src/hasp.rs`): HASP4 `hasp()` answered in memory. Memory:
  `PARLST1.0CTRYUS\0CAB1` then 0xff. Byte 0x0f is the cabinet type (`g_arcadecabinet`: '0' /
  0 Single, '2' Deluxe, '1' Standard). Read back from a working setup with
  `tools/dongle-recording/farcry-dongle-dump.py` (struct at `0xa5278c`).
- **Dongle checks**: with the original executable the emulated memory passes the boot and
  level checks (`0x4bd980` compares "PARLST1" and "1.0"). The "IncorrectDongle" seen before
  came from the crack, which inverted those checks: its patches are no longer needed.
- **PC mouse**: the input manager (`0x7af990`, only while the window has focus) updates the
  keyboard (`0x7abab0`), the mouse (`0x7ac3b0`, FPS mouse look in levels) and the USBIO guns
  (`0x7af480`). The mouse update is removed (`0x7af9a1`, 18 bytes nop). Forcing "no focus"
  instead (`0x403e9b` = 0) also stops the guns: crosshair frozen in levels.
- **PC keyboard** (`payload-globalvr/src/keyboard.rs`): the console game's controls and debug
  keys (WASD walk, G god mode, F1-F5 difficulty...) come from DirectInput (`0x7abaf3`) and
  window messages; only test / service / up / down reach the game.
- **USBIO version**: the game looks for the board over HID (VID 0x1ab7 or 0x04b4, PID 0x7013)
  for its firmware version; "n/a" raises `g_arcadeError` 0x400 every frame (controller test
  shows the trigger BAD). `0x7af4b6` je -> nop.
- **Gun watchdog**: a gun whose input manager timestamp (+0x388 / +0x38c) is 60 s old raises
  `g_arcadeError` 0x800 / 0x1000 (`0x5c5e4f`, `0x5c7f20`; set with `0x4be910`, cleared with
  `0x4be960`), red marks in the screen corners. The virtual guns hold still:
  `0x5c5e56` / `0x5c7f27` jbe -> jmp.

## Ruled out

Frame rate (60 fps cap), CPU (one core; no 3DNow!, no vendor or CPU count check), timer (QPC
deltas), `p_max_substeps`, missing files (strace), tutorial videos (Indeo 5 AVIs replaced:
same), Lua errors (`WAL_GLOBALVR_GAMELOG=1` hooks the script and Lua error paths: none),
swallowed exceptions (`WINEDEBUG=+seh`: none), XML (static expat).
2026-10-30, each a full level run, same result: the original executable instead of the crack,
the gun error bits gone, wined3d instead of DXVK, Wine's builtin dsound instead of the native
one (the game imports dsound: it does not start without), cabinet type Deluxe
(`g_arcadecabinet` 2, `0x4bd9ff` push 1 -> push 2; Standard 3 already tried), every heap block
zeroed (`RtlAllocateHeap` + `HEAP_ZERO_MEMORY`), Windows XP version, x87 precision (DXVK's
CreateDevice already sets 24 bit, cw 0x27f -> 0x7f, behavior 0x44, as Windows' d3d9).
The game's message functions (`WAL_GLOBALVR_GAMELOG=1`) print nothing in the level.

## What happens at level start (traced 2026-10-30)

Tools (test payload only, not in the repo):

- `lua.dat` / `lua.fat` / `lua.nfo`: `DAT2`, entry count, then 12-byte entries (offset, size,
  name hash); the `.nfo` holds each name right after its hash. The 445 scripts are plain
  Lua 4.1 chunks (no compression), float numbers (Xbox build). The arcade levels'
  own scripts only load sound banks: the level logic is entities (triggers, generators,
  mounted weapons) in the level `.dat`.
- Script → C++ calls: every script class has a thunk that calls `0x879660`
  (`IFunctionHandler` function id) then its member table; ids map to names by the order
  of the class's register helper calls (`push func; push name; call helper`, helper ends
  with `call [edx+0x114]`). Entity class = return address `0x422684`, Game `0x541fc4`,
  Player `0x575d84`.
- Engine → script calls by name: `BeginCall(table, func)` `0x877b10` (the call by function
  reference is `0x877a50`). Missing functions are silent (the error needs a flag).
- `System:Log` / `LogToConsole` (`0x429c40` / `0x429c00`) do nothing in this build.
- `Entity:SetPos` → `IEntity::SetPos` call at `0x415b65`, `Entity:GetPos` result at
  `0x415439`; the entity name is the `std::string` at `IEntity + 0xa8` (msvcp71: text at
  +4, size +0x14, capacity +0x18).

Trace of Arcade1_Island (free play, trigger pressed):

- Level loaded, the player is spawned at the first respawn point (841.1, 10.1, 26.8).
- Intro fly-over (~16 s), it ends behind the M16 gun on the beach sandbags.
- The mount **does** happen: `Game:ArcadeStartMission`, `Player:EnterMountedWeapon`,
  `Game:ArcadeChangeCameraMode(1)` (`0x53c4a0`: sets `r_arcadecamera`, player +0xeb4 = 2
  (camera snap frames), player +0x6e0 = 0), `RedirectInputTo`, then the gun's update puts the
  player behind the gun every frame.
- Gun `MountedWeapon0_M16G` at (692.31, 181.49, 16.02); player set to
  (691.82, 180.86, 14.92): 0.8 m behind, feet 1.1 m under the gun. Merc spawn spots around
  there are at z 15.4 to 16.4 (the beach), boat spawn spots at 14.5 to 15.8 (the sea).
- 2 s after the mount the view looks the same way as the end of the fly-over (tower and pier
  on the right) but from lower: the sand is gone (terrain seen from below is not drawn),
  only the sea plane shows. So the camera is under the ground / at sea level, and the health
  drain is probably drowning: the player's feet (14.92) are under the sand and likely under
  the water level, and `Player:IsUnderWater` is polled.
- Level water level is 15 (`WaterLevel="15"` in the level `.dat`); every gun has
  `mountHeight="1.1" mountRadius="0.8"`.

### Cause: the "Cam First" bone is not animated

- The local player's eye update (`0x4fe0xx`-`0x4fe9xx`) takes the eye from a bone matrix:
  "Cam First" (`CPlayer + 0x8d0`) when vfunc +0x70 is true (first person, mounted), else
  "Head" (`+0x8e4`). Bone pointers are set at `0x4eacfa` (names "Cam First", "Root Control",
  "Weapon Left", "Weapon Right", "MasterRoot", "Head"). A bone is 0xb0 bytes: relative
  matrix at +0x18, model space matrix at +0x5c (translation +0x0c / +0x1c / +0x2c), parent at
  +0x9c; `0x756130` loop / `0x759960` computes model = parent * relative every frame.
- The eye (world) is stored at `CPlayer + 0x31c`; `0x4dd0f0` / `0x4fe59e` set the underwater
  flag (`+0x540`) when the water level is above the eye z.
- Logged on the mounted gun: "Cam First" stays at (-0.02, -0.28, **0.04**), the same every
  frame; "Head" is animated (1.58 m standing, 0.13 m once the swim pose starts). Eye world z
  14.96 = 4 cm above the feet, under the water: camera at sea level under the sand, drowning,
  the gun never fires (0 `GameRules:OnWeaponFire`).
- Hardware watchpoints: Head's relative matrix is written by the animation code (`0x76532b`)
  and `0x5045dc`; **nothing ever writes Cam First's relative matrix**. Only the camera reads
  `+0x8d0`. So no playing animation drives "Cam First"; why it is driven on Windows is not
  known (first person anim tables are registered: `RegisterFPAnimTable`).
- Tests: player raised 1.5 m (`SetPos` z) → beach view, no drowning. Camera on "Head"
  (`0x4fe2b7` je → jmp) → right height but the camera takes the head's rotation (tilted).
  Eye z + 1.5 when it comes from an unanimated "Cam First" → beach view, health stays 100,
  the gun fires and the mission ends: "SHOTS FIRED 102", score 8000, flawless victory medal.

### Workaround (removed)

`payload-globalvr/src/mounted_eye.rs`: hook `0x4fe2bd` (marks the "Cam First" branch) and
`0x4fe58e` (after the eye is stored, before the underwater test): when the eye came from
"Cam First" and that bone's model z is under 0.3 m, eye z += 1.5. Bytes are checked first.

## Missing crates / barrels / chairs and frozen enemies (solved: see root cause)

User report: the enemies are statues (no movement, no shooting; shot, they die and fall);
compared with a video of a cabinet, the beach in front of the first gun should have many
wooden crates and petrol barrels (the fire comes from a shot barrel) and has none.

- The missing objects are all `Type="PrototypeEntity"` entities (`DestroyableObject` /
  `BasicEntity` with only `tplCreatureType="PhysObj.Crate.WoodMed_NoKrieger"`...); the
  `Type="Entity"` ones with their own properties (`DestroyableObject0`, the sniper tower)
  are drawn. Merc generators also use prototypes (`EnemyArchetypesLibrary.Merc2.Scout_Glock`).
- Prototype resolution works: `0x597530` (entity from XML) → lookup `0x8b74b0` (library
  `0xa52cd8`, `_stricmp` on `Name`) finds every prototype, and the XML node handed to the
  Lua `Properties` table (`0x597766` → `0x5964a0`) has all 44 prototype attributes.
- `Entity:LoadObject` (`0x4110d0` → `CEntity::LoadObject 0x4a2940`) succeeds for the crate
  and barrel models (and their destroyed models, preloaded at init: not a destruction).
- Memory scan of every entity (vtable `0x905638`, name std::string +0xa8, position +0x34c,
  flags +0x18): the crates exist at their editor positions (`WharfCrate56` at
  (702.2, 195.0, 15.0) in front of the gun) but have radius 0 (+0x3d4), no object (+4 = 0)
  and an empty box, so they are never registered with the 3D engine (`0x489f90` needs a
  radius and +4; flag 0x20000 = registered). Drawn entities have the flag, a radius and a
  box. Two objects are far away (`WharfCrate47` at (93239, 240, 9503), `WharfBarrel2` z 66).
- Ruled out (no change): `e_hw_occlusion_culling_objects 0`, `e_on_demand_physics 0`,
  `e_stream_cgf 0` + `e_stream_for_visuals 0` + `e_stream_for_physics 0` (cvars added to
  `systemcfg.lua`, which the game reads at start), `LC_ALL=C`.
- Next: find where a loaded object becomes the entity's object (+4) and its box (after
  `0x4a2940`), and what clears it for these entities (watchpoint on +4 / +0x3d4 of a crate).

## Frozen enemies (solved: see root cause)

With the eye fix the player can shoot (user confirmed), but the enemies stay in place: in a
full headless mission they never hit the player (health stays 100).

- AI scripts run: `AI:PushGoal` 1000+ calls, `FindRandomAnchorInRange`, `Signal`,
  `RegisterWithAI`; no `[AIWARNING]` / `[AIERROR]` (they go through `0x726f70` →
  `0x4031c0`, hooked by `WAL_GLOBALVR_GAMELOG`).
- Animations play: skeleton update `0x7560d0` (bones at skeleton +0xc0, 0xb0 each) sees the
  62-bone character skeletons change pose on every update.
- Entities: `IEntity` vtable `0x905638`, `SetPos` `0x493e60` (+0x10), vfunc +0x14
  `0x489450` returns +0x358 / +0x388 (angles, not the position). Only the player gets
  `SetPos`.
- Physical world: global `0x992ef8` (vtable `0x924234`), variables at world +0x10
  (`p_single_step_mode` +0x30, `p_do_step` +0x34, `p_fixed_timestep` +0x38,
  `p_max_substeps` +0x5c = 5, gravity +0x68 = -9.8): normal values at run time.
- Theory to check: the living entities (player, AIs) are not moved by the physics (the
  player left inside the sand was never pushed out either). Next: find the world's
  `TimeStep` and log its time step, and read an AI's physical position over time.

## Leads

- The mounted gun itself is not drawn (`r_arcadedrawmountedweapon` 0 by default): check it
  is the same on a cabinet.
- Second HID board (PID 0x7015 / 0x7012, string at input manager +0x394): outputs / force
  feedback.
