//! Sega ALLS APM3 ("ALL.Net P-ras MULTI Version 3": Under Night In-Birth Exe:Late[cl-r], Blade
//! Strangers, Senxin Aleste, Otoshu DX, Kasiori, Nosferatu Lilinor, Pengo! Online, Puyo Puyo
//! e-Sports, Milkchan, Koihime Enbu RyoRaiRai Version 3, Goonya Fighter, Guilty Gear -Strive-,
//! Dead or Alive 6).
//!
//! The game imports `apm_x86.dll` (`apm.dll`: 64-bit games and programs), the client library of the cabinet's APM3 system
//! (`amdaemon`: keychip, ALL.Net, I/O board, Aime, credits): our payload replaces it and
//! implements the API itself. The test program (`TestModeApp.exe`) imports it too.
//!
//! Payload options (profile `env`, details in `crates/payload-apm3`):
//! - `WAL_APM3_ONLINE=1`: ALL.Net and the Aime server reported up: Aime cards, the players' data
//!   on the payload's title server (`apms/<aime id>.dat` in the game folder);
//!   `WAL_APM3_ONLINE_PATCHES`: game patches for it (as `WAL_PATCHES`). Default offline.
//! - `WAL_APM3_ACCESS_CODE` (20 digits), `WAL_APM3_AIME_ID`: the card (`card` input);
//!   `WAL_APM3_CARD_AUTO=1`: always on the reader (the `card` input takes it off)
//! - `WAL_APM3_ABAAS_LINK`: the ABAAS matching server (cabinet versus; none emulated yet); without
//!   it `WAL_APM3_NO_LINK_PATCHES` apply (the game no longer waits for its link)
//! - `WAL_APM3_FREEPLAY=0`: coins, 1 credit per game (default: free play)
//! - `WAL_APM3_TEST`: the test program the game's test switch starts (default `TestModeApp.exe`);
//!   `WAL_APM3_TEST_ARGS`: its arguments (a test mode of the game's own program: `-t`);
//!   `WAL_APM3_TEST_CWD`: its working folder in the game directory (default: the game's)
//! - `WAL_APM3_JOYSTICK=1`: the switches read as winmm joystick 0, the I/O board's buttons (Tapping
//!   Skill Test); `WAL_APM3_JOY_MAP` `<native>:<bit>,...` over the default map
//! - `WAL_APM3_LINK_READY=0`: `abaaslink.dll`'s link left unavailable (default: reported
//!   available, no "matching unavailable" warning)
//! - `WAL_APM3_TOUCH=1`: a touch screen game (player 1's gun and `touch` input; `pinch_in` /
//!   `pinch_out`: mouse wheel notches, the pinch of Tapping Skill Test's window)
//! - `WAL_APM3_GS_KEY`, `WAL_APM3_GS_MODULE`: the ABAAS game server's key (default: learnt),
//!   the DLL of its client (default `abaasgs.dll`); `WAL_APM3_GS_ONLINE=1`: the player loaded
//!   online (Dead or Alive 6 saves the player only then)
//! - `WAL_APM3_KEYCHIP`, `WAL_APM3_BOARD`, `WAL_APM3_GAME_ID`, `WAL_APM3_LOCATION`,
//!   `WAL_APM3_RESOLUTION` (`<w>x<h>`): values the system reports
//! - `WAL_APM3_PLAYER_NAME`: the player's name on Guilty Gear -Strive-'s server (default `PLAYER`)
//! - `WAL_APM3_APMS_TRACE=1`: the title server's AES key and plain bodies in the log
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`

use super::{Payload, System};

pub struct Apm3;

impl System for Apm3 {
    fn name(&self) -> &'static str {
        "apm3"
    }

    fn payloads(&self) -> &'static [Payload] {
        // apm.dll: the system's 64-bit library, for the game's 64-bit programs (Blade Strangers'
        // test menu, a .NET AnyCPU program)
        &[
            Payload { file: "wal_apm3.dll", install_as: "apm_x86.dll" },
            Payload { file: "wal_apm3_64.dll", install_as: "apm.dll" },
        ]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[
            Payload { file: "wal_apm3_64.dll", install_as: "apm.dll" },
            Payload { file: "wal_apm3.dll", install_as: "apm_x86.dll" },
        ])
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        // the Aime card players' records (the payload's title server), the system's backup
        &["apms", "backup"]
    }
}
