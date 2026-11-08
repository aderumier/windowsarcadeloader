//! The ABAAS game server ("GS", `am::abaas::GsMain`: Blade Strangers' player data), served on
//! the payload's title server port (`apms.rs`: the name is 127.0.0.1, port 80 moved there),
//! routed by the `/api/` path: payload-common's `abaas_gs` (the players' files in `apms\`, the
//! options `WAL_APM3_GS_KEY`, `WAL_APM3_GS_SEQ`).

use wal_payload_common::abaas_gs;

fn configure() {
    abaas_gs::configure("apms", "WAL_APM3");
}

/// The title key from the game's `BCryptGenerateSymmetricKey` call (apms.rs).
pub(crate) fn learn_key(secret: &[u8]) {
    configure();
    abaas_gs::learn_key(secret);
}

/// The reply body to one request (`path` from `/api/`).
pub(crate) fn reply(path: &str, body: &[u8]) -> Vec<u8> {
    configure();
    abaas_gs::reply(path, body)
}
