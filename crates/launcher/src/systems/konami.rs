//! Konami e-Amusement PC games (Nobiteke! Byon Byon Daisakusen, Gashaaaan, Gashaaaan Refill!, Otomedius,
//! Winning Eleven 2008, Winning Eleven 2012, GTI Club: Supermini Festa!, Road Fighters 3D,
//! Contra: Evolution, Wartran Troopers, Chase Chase Jokers, Lethal Enforcers 3, Cooper's 9,
//! Castlevania: The Arcade, Silent Hill: The Arcade, Action Deka, Hyper Sports 2011 Remake,
//! The Bishi Bashi, Thrill Drive 4: Crazy Streets, Hello! Pop'n Music, Rainbow Bomber Girl).
//!
//! The game drives its cabinet through `libacio.dll`, Konami's ACIO bus driver (HBHI I/O board:
//! switches, analog controllers, coins, security plugs; ICCA card reader): our payload replaces
//! it, so the game loads it through its own imports.
//!
//! The payload also answers the cabinet's network setup (`SetAdapterIpAddress`), runs the game
//! with e-Amusement offline (the boot's connection check takes its offline branch) and gives the
//! picture the screen's resolution in a borderless window.
//!
//! Gashaaaan Refill! has no ACIO: its profile installs the payload as `libbms.dll` (the
//! BM-System client), which answers the game's own I/O library, ig2ACLib (docs §6.3h).
//! Castlevania: The Arcade's profile installs it as `libbms.dll` too: it answers the game's
//! hcvACLib (buttons, coins) and turns each player's gun into a whip (docs §6.3h).
//! Silent Hill: The Arcade's profile installs it as `libbms.dll` too: it points the game's
//! shaiolib (buttons, guns, coins, security plug) at itself (docs §6.3h).
//! Action Deka's payload is its libacio (GDKI controllers, card reader) and answers its libbms,
//! its statically linked EZ-USB library (switches, coins) and, from the players' guns, its own
//! debug input (aim, trigger, motions, steering; docs §6.3h).
//! The Bishi Bashi's payload is its libacio (HBHI board: three players' red, green, blue and
//! yellow buttons) and answers its libbms (docs §6.3h).
//! Hyper Sports 2011 Remake imports no I/O library: its profile starts it through wal-loader
//! (`payload_loader`, installed as `python4.dll`); the payload answers the game's gf.dll
//! imports of its Python 4 I/O board and LED controller and gives the switches to gf.dll's
//! input (docs §6.3h).
//! Gashaaaan's profile installs it as `iob.dll` (its I/O board library), which also answers
//! its camera board library (ballio) and shows its two screens as one picture.
//! Otomedius's libacio.dll is the payload too (PJEI board), which also answers the game's libbms
//! and libtouchpanel imports; Winning Eleven 2008's too (PJEI board, PS2 pad port, touch panel).
//! GTI Club: Supermini Festa! also loads it as `dllibacio.dll`, its second ACIO bus (HGTH board:
//! wheel, pedals, the wheel's motor, its torque and vibration the force feedback).
//! Thrill Drive 4's libacio.dll is the payload (PJEI board: switches, wheel and pedals), which
//! the game also loads as `libtd4io.dll`, its second ACIO bus (D44H board: the wheel's motor,
//! its torque and vibration the force feedback); the payload passes the boot's motorized wheel
//! calibration and skips its data check.
//!
//! Contra: Evolution imports libacio's functions under their MSVC names and never calls them:
//! the payload answers its COM1 control panel (coins) and its DirectInput keyboard (the players'
//! controls, from the virtual sticks), and reports its operator clock set.
//!
//! Road Fighters 3D's code is a DLL (`jgt.dll`, the dump's `.windowsloader`): `wal-avs-boot.exe`
//! runs it as the cabinet's launcher does (AVS, e-Amusement boot, the DLL's entry points); its
//! `libacio.dll` and `libacioEx.dll` (the HGTH wheel board's bus) are the payload. Hello! Pop'n
//! Music's too (`PMU/popn.dll`: HBHI board, two players' OK and four buttons). Winning
//! Eleven 2012's too (`contents/modules/weac12_bootstrap_release.dll`, AVS 2.13: PJEI board, PS2
//! and PS3 pad ports, touch screen).
//!
//! Wartran Troopers has no ACIO: the payload is its `panicdialog.dll` and answers its serial
//! I/O chain (COM2: hub, boards, security plugs) and gun boards (COM1), its two monitors shown
//! side by side in one window.
//!
//! Chase Chase Jokers (Unity, 64-bit) drives its I/O board through Konami's AIO libraries: the
//! 64-bit payload (`wal_konami64.dll`) is installed as `libaio.dll`, `libaio-iob.dll` and
//! `libaio-iob2_video.dll` (its profile's `payload_install`), and answers its raw input
//! trackball and touch screen.
//!
//! Rainbow Bomber Girl (Unity, 64-bit) has no cabinet library in its dump: its profile installs
//! the 64-bit payload as the `winhttp.dll` UnityPlayer imports. It answers the game's BM-System
//! library (credits, switches, watchdog) and I/O board through Mono, its touch screen, and shows
//! its two monitors (drawn as the cabinet's 2160x1920 desktop) side by side on the screen.
//!
//! Bishi Bashi Channel's code is a 64-bit AVS2 DLL (`contents/modules/bsch.dll`): run by
//! `wal-avs-boot64.exe` (installed as `wal-avs-boot.exe`), the 64-bit payload is its
//! `libacio.dll` and `libacio_pix.dll` (BMPU and PIX boards: four players' buttons and disks).
//!
//! Cooper's 9's executable is a memory dump without imports: its profile starts it through
//! wal-loader (`payload_loader`) with the payload as `inpout32.dll` (its parallel port I/O
//! board), which rebuilds the imports and answers the game's HASP dongle.
//!
//! Payload options (profile `env`):
//! - `WAL_KONAMI_CARD_ID`: the e-Amusement card's id, 16 hex digits
//! - `WAL_KONAMI_AVS_LOG=1`: the AVS log (the game system's boot, errors) in the payload log
//! - `WAL_KONAMI_CROSSHAIRS=1` (`_STYLE`, `_SIZE`, `_P1`..`_P4`, `_TINT`): the players' aim drawn
//!   over the picture (Gashaaaan, Gashaaaan Refill!)
//! - `WAL_KEEP_ASPECT_RATIO=0` (Gashaaaan): the picture stretched to the screen (`1`: aspect
//!   kept), Batocera ES's KEEP ASPECT RATIO
//! - Winning Eleven 2008: `WAL_KONAMI_GAME_LOG=1` the game's own log in the payload log;
//!   `WAL_KONAMI_EAMUSEMENT=1`, `WAL_KONAMI_LIVE_MONITOR=1` keep those test menu network options
//!   (forced off); `WAL_KONAMI_DDRIVE` the cabinet's D: folder (default the dump root)
//! - wal-avs-boot: `WAL_KONAMI_AVS_HEAP_MB` the AVS heaps' size (default 16), `WAL_KONAMI_DEST`
//!   the destination (region) in place of the dump's; Winning Eleven 2012: its region (`J`
//!   Japan, `A` Asia, `C` China) and the cabinet's product code
//! - GTI Club, Thrill Drive 4: `WAL_KONAMI_FFB` the force feedback strength, percent (default
//!   100)
//! - Wartran Troopers: `WAL_KONAMI_PLUG` the game plug's region id (default `GKD00UA`)
//! - Action Deka: `WAL_KONAMI_UNLOCK_FINAL=1` the 4th mission offered at once (default)
//! - Silent Hill: The Arcade: `WAL_KONAMI_REGION` the security plug's region (`E`, `U`, `A`,
//!   `K`, `J`; default `E`), `WAL_KONAMI_SPEAKERS` the speaker channel (`2`, `5.1`)
//! - Castlevania: The Arcade: `WAL_KONAMI_UNLIMITED_SUBWEAPON=1` sub-weapons cost no hearts,
//!   `WAL_KONAMI_WHIP_SHOT=0` drags with the trigger held swing the whip
//! - Cooper's 9: `WAL_KONAMI_REGION` the dongle's region (`J`, `U`, `A`, `K`, `E`; default
//!   `E`), `WAL_KONAMI_SAVE_DRIVE` the drive of the settings files, `WAL_KONAMI_GUN_FLASH=1` the
//!   light gun flash at each shot
//! - Bishi Bashi Channel: `WAL_KONAMI_EAMUSEMENT=1` keeps the test menu's e-amusement setting
//!   (set off)
//! - Chase Chase Jokers: `WAL_KONAMI_TRACKBALL_SPEED` the trackball's counts per 10 ms at the
//!   right stick's full tilt (default 12)
//! - Rainbow Bomber Girl: `WAL_KONAMI_PRICE` credits per game (default 1),
//!   `WAL_KONAMI_FREEPLAY=1` free play, `WAL_KONAMI_REGION=asia` the export version,
//!   `WAL_KONAMI_LAYOUT` the screens (`main_left`: the main monitor on the left, no bezel;
//!   `raw`: the game's own picture)
//!
//! Native inputs for `native_map`: the board's switches `test service start trigger button2`
//! and `coin` (players 1 and 2 have their own start, trigger and button2); Gashaaaan and Gashaaaan
//! Refill!: `test service coin start throw` (players 1-4); Otomedius: `test service coin start up down
//! left right a b c`; Winning Eleven 2008: also `d e f`, and the PS2 pad `pad_start select up down
//! left right triangle cross square circle l1 l2 l3 r1 r2 r3`; GTI Club: `test service coin start
//! action shift_up shift_down select_up select_down select_left select_right`; Thrill Drive 4:
//! `test service coin start view shift_up shift_down`; Contra: Evolution:
//! `start up down left right shot jump select test service coin` (players 1 and 2); Wartran
//! Troopers: `start trigger option shoulder sense offscreen` (players 1-4), `test service coin`;
//! Cooper's 9: `test service coin start trigger offscreen` (players 1 and 2); Action Deka:
//! `test service coin start trigger punch guard throw special reload steer_left steer_right`
//! (players 1 and 2); Hyper Sports 2011 Remake: `test service coin red blue green` (players 1
//! and 2); Silent Hill: The
//! Arcade: `test service coin start trigger offscreen option_l option_r` (players 1 and 2); Castlevania: The
//! Arcade: `test service coin start attack switch whip subweapon select` (players 1 and 2); Chase Chase
//! Jokers: `test service coin dash skill attack jump slide headphone up down left
//! right touch`; Bishi Bashi Channel: `test service coin red green blue disk_left disk_right`
//! (players 1-4); The Bishi Bashi: `test service coin red green blue yellow` (players 1-3);
//! Hello! Pop'n Music: `test service coin start button1 button2 button3 button4` (players 1
//! and 2); Rainbow Bomber Girl: `test service coin bomb skill1 skill2 back detonate headphone up
//! down left right touch`.

use super::{Payload, System};

pub struct Konami;

impl System for Konami {
    fn name(&self) -> &'static str {
        "konami"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_konami.dll", install_as: "libacio.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        // Chase Chase Jokers: its profile installs it as the AIO libraries (payload_install),
        // Rainbow Bomber Girl as winhttp.dll
        Some(&[Payload { file: "wal_konami64.dll", install_as: "libaio-iob2_video.dll" }])
    }

    fn dll_host(&self) -> Option<&'static str> {
        Some("wal-avs-boot.exe")
    }
}
