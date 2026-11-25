//! The cabinet's card dispensers (A: the printed card each play hands out, B: Hero Licenses),
//! read through neodcd's `NeoDcdDispenser*` functions: their imports in the game answer a
//! healthy dispenser and every payout done (the cards are the binder's here). neodcd's own
//! dispenser module never starts (it talks to them over the I/O board's serial channel,
//! error 22-4 without them).

use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::{iat, log};

const INIT_COMPLETED: i32 = 2;
const STATUS_OK: i32 = 1;
const PAYOUT_OK: i32 = 1;
const PAYOUT_UNKNOWN: i32 = 0;

/// The dispenser of the last payout (its result read once).
static PAID: AtomicI32 = AtomicI32::new(-1);

pub fn init(exe: usize) {
    let hooks: [(&str, usize); 17] = [
        ("NeoDcdDispenserInit", one0 as *const () as usize),
        ("NeoDcdDispenserGetInitStatus", init_status as *const () as usize),
        ("NeoDcdDispenserGetStatus", status as *const () as usize),
        ("NeoDcdDispenserSetSeries", none2 as *const () as usize),
        ("NeoDcdDispenserGetSeries", zero1 as *const () as usize),
        ("NeoDcdDispenserResetPayoutTable", none0 as *const () as usize),
        ("NeoDcdDispenserGetPayoutResult", payout_result as *const () as usize),
        ("NeoDcdDispenserPayout", payout as *const () as usize),
        ("NeoDcdDispenserClearBackupedPayouts", none0 as *const () as usize),
        ("NeoDcdDispenserPeekPayoutBackup", zero2 as *const () as usize),
        ("NeoDcdDispenserDecPayoutBackup", zero2 as *const () as usize),
        ("NeoDcdDispenserGetBackupedPayouts", zero0 as *const () as usize),
        ("NeoDcdDispenserIsEmpty", zero1 as *const () as usize),
        ("NeoDcdDispenserReset", one1 as *const () as usize),
        ("NeoDcdDispenserIsJammed", zero1 as *const () as usize),
        ("NeoDcdDispenserIsBusy", zero1 as *const () as usize),
        ("NeoDcdDispenserIsDisconnected", zero1 as *const () as usize),
    ];
    for (name, f) in hooks {
        if unsafe { iat::hook_module(exe, "neodcd.dll", name, f) }.is_none() {
            log!("sdbh: the game does not import {name}");
        }
    }
}

unsafe extern "system" fn init_status() -> i32 {
    INIT_COMPLETED
}
unsafe extern "system" fn status(_dispenser: i32) -> i32 {
    STATUS_OK
}
unsafe extern "system" fn none0() {}
unsafe extern "system" fn none2(_a: i32, _b: i32) {}
unsafe extern "system" fn zero0() -> i32 {
    0
}
unsafe extern "system" fn zero1(_a: usize) -> i32 {
    0
}
unsafe extern "system" fn zero2(_a: usize, _b: usize) -> i32 {
    0
}
unsafe extern "system" fn one0() -> i32 {
    1
}
unsafe extern "system" fn one1(_a: usize) -> i32 {
    1
}

/// `Payout(use)`: 1 a game's card, 2 a bought Hero License (dispenser B).
unsafe extern "system" fn payout(usage: i32) -> i32 {
    let dispenser = if usage == 2 { 1 } else { 0 };
    log!("sdbh: dispenser {} pays out a card (use {usage})", if dispenser == 1 { "B" } else { "A" });
    PAID.store(dispenser, Ordering::Relaxed);
    1
}

unsafe extern "system" fn payout_result(dispenser: *mut i32) -> i32 {
    let paid = PAID.swap(-1, Ordering::Relaxed);
    if paid < 0 {
        return PAYOUT_UNKNOWN;
    }
    if !dispenser.is_null() {
        unsafe { *dispenser = paid };
    }
    PAYOUT_OK
}
