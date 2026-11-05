//! UNIS: UNIS's PC-based arcade games (Dino Invasion, After Dark, Night Hunter, Raccoon
//! Rampage, Crazy Ride, Elevator Action Invasion, Fantasy Soccer, Crazy Speed Arcade, Wicked
//! Tuna, Mighty Moto).
//!
//! Dino Invasion (Unity 5.4, Mono, 64-bit; the "shunxiang" gun cabinet): our payload is the
//! `xinput1_3.dll` Unity loads at start; it emulates the COM3 I/O board (coins, triggers, the
//! operator's keys), the guns' camera tracker (`LC64.dll`) and the dongles, the real keyboard
//! and mouse filtered (docs §6.3j).
//!
//! After Dark (Virtools 3.5, 32-bit): started by `wal-loader` with the 32-bit payload, which
//! emulates its COM1 gun board and its HID board's device object (the dongle).
//!
//! Night Hunter: After Dark Chapter II (Unity 5.5, Mono, 64-bit; the "qumo" gun cabinet): the
//! payload emulates its USB HID gun board and its Sense4 dongle.
//!
//! Raccoon Rampage (Unreal Engine 4, 64-bit): the game imports `xinput1_3.dll`; the payload
//! answers its `UsbPluginsDll.dll` (I/O board, coins, dongle) and maps the guns on the picture;
//! Crazy Ride (the same, `UsbPluginsDll_R.dll`): its I/O board, wheel and coins; Elevator
//! Action Invasion (the same DLL): its I/O board, USB guns, coins and the elevator doors, drawn
//! over the picture; Fantasy Soccer (a packed executable, `UsbPluginsDll_R.dll`): its four
//! players' sticks, buttons and balls, coins.
//!
//! Wicked Tuna (Unity 2018, Mono, 64-bit): the payload emulates its four rods' serial board,
//! the rods on light guns (or sticks).
//!
//! Mighty Moto (Unity 2.x, 32-bit): started by `wal-loader` with the 32-bit payload, which
//! gives the game its cabinet's keys from player 1's stick.
//!
//! Crazy Speed Arcade (Ogre, 32-bit): started by `wal-loader` with the 32-bit payload, which
//! emulates its COM1 "game232" driving board and disarms its dongle, machine id and network.
//!
//! Native inputs for `native_map`: Dino Invasion's `trigger start coin` (every player) and the
//! operator's `menu select`; After Dark's `start trigger platform test plus minus coin` (each
//! gun); Night Hunter's `trigger big change coin stop` (each gun), `test service reset`;
//! Raccoon Rampage's `trigger coin` (each gun), `test down enter`; Crazy Ride's `start pause
//! coin`, `test down enter`; Elevator Action Invasion's `trigger p1start coin` (each gun),
//! `gunmiddle gunright gunup gundown gunleft gunright2` (the gun's buttons), `test down enter`;
//! Fantasy Soccer's `kick pass accelerate switch button2 coin` (each player), `test down enter`;
//! Wicked Tuna's `hook reel coin` (each rod), `test service repair`; Mighty Moto's `start item
//! coin test gas brake back handbrake left right menuleft menuright menuenter`; Crazy Speed
//! Arcade's
//! `button0`..`button21` (5 start, 4 view) and
//! `coin`.

use super::{Payload, System};

pub struct Unis;

impl System for Unis {
    fn name(&self) -> &'static str {
        "unis"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_unis32.dll", install_as: "xinput1_3.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_unis.dll", install_as: "xinput1_3.dll" }])
    }
}
