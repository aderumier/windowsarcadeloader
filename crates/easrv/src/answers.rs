//! The fake e-Amusement server's answers (spice2x's, easrv.cpp), as kbin documents.

use crate::kbin::{Node, Value};

/// The services every request asks for, all at this server.
const SERVICES: &[&str] = &[
    "cardmng", "facility", "message", "package", "pcbevent", "pcbtracker", "posevent", "pkglist", "dlstatus", "eacoin", "lobby",
    "lobby2", "local", "local2", "apsmanager", "netlog", "userdata", "eventlog", "matching", "system", "hdkoperation",
];

/// Winning Eleven 2008's services (its xrpc modules).
const SERVICES_HCK: &[&str] = &[
    "cardmng", "clientlog", "facility", "goalreplay", "message", "netconfig", "package", "pcbevent", "pcbtracker", "playerdata",
    "posevent", "ranking", "sidmgr", "system", "takeover", "local",
];

fn s32(name: &str, v: i32) -> Node {
    Node::value(name, Value::S32(v))
}

fn getmaster(strdata1: &str) -> Node {
    Node::new("response").child(
        Node::new("system")
            .child(s32("result", 1))
            .child(Node::str("strdata1", strdata1))
            .child(Node::str("strdata2", "MSwxLDEsMSwxLDEsMSwxLDEsMSwxLDEsMSwx"))
            .child(Node::value("updatedate", Value::U64(1120367223))),
    )
}

/// `(name, document)`: `<module>.<method>`, or `<model>.<module>.<method>` for one game's
/// answer, `empty` for anything else. `url`: this server's services URL.
pub fn answers(url: &str) -> Vec<(&'static str, Vec<u8>)> {
    let keepalive = format!("{url}keepalive?pa=127.0.0.1&ia=127.0.0.1&ga=127.0.0.1&ma=127.0.0.1&t1=2&t2=10");
    let services = Node::new("response").child(
        Node::new("services")
            .children(SERVICES.iter().map(|s| Node::new("item").attr("name", s).attr("url", url)))
            .child(Node::new("item").attr("name", "ntp").attr("url", "ntp://pool.ntp.org/"))
            .child(Node::new("item").attr("name", "keepalive").attr("url", &keepalive)),
    );
    let facility = Node::new("response").child(
        Node::new("facility")
            .child(
                Node::new("location")
                    .child(Node::str("id", "JP-01"))
                    .child(Node::str("country", "JP"))
                    .child(Node::str("region", "."))
                    .child(Node::str("name", "LOCAL"))
                    .child(Node::value("type", Value::U8(0))),
            )
            .child(Node::new("line").child(Node::str("id", ".")).child(Node::value("class", Value::U8(0))))
            .child(
                Node::new("portfw")
                    .child(Node::value("globalip", Value::Ip4([127, 0, 0, 1])))
                    .child(Node::value("globalport", Value::U16(8888)))
                    .child(Node::value("privateport", Value::U16(8888))),
            )
            .child(
                Node::new("public")
                    .child(Node::value("flag", Value::U8(1)))
                    .child(Node::str("name", "."))
                    .child(Node::str("latitude", "0"))
                    .child(Node::str("longitude", "0")),
            )
            .child(
                Node::new("share")
                    .child(
                        Node::new("eacoin")
                            .child(s32("notchamount", 0))
                            .child(s32("notchcount", 0))
                            .child(s32("supplylimit", 1000000)),
                    )
                    .child(Node::new("url").children(
                        ["eapass", "arcadefan", "konaminetdx", "konamiid", "eagate"].map(|n| Node::str(n, "http://localhost")),
                    )),
            ),
    );
    // Winning Eleven 2008: its services only, at the server's root (the answer is read into the
    // game's 4 KB `sz_xrpc_buf`: the full list does not fit, "read error by mem")
    let root = url.trim_end_matches("service/");
    let services_hck = Node::new("response").child(
        Node::new("services")
            .children(SERVICES_HCK.iter().map(|s| Node::new("item").attr("name", s).attr("url", root)))
            .child(Node::new("item").attr("name", "keepalive").attr("url", format!("{root}keepalive?pa=127.0.0.1&ia=127.0.0.1&ga=127.0.0.1&ma=127.0.0.1&t1=2&t2=10"))),
    );
    let docs = [
        ("services.get", services),
        ("HCK.services.get", services_hck),
        (
            "pcbtracker.alive",
            Node::new("response").child(
                Node::new("pcbtracker")
                    .attr("ecenable", 0)
                    .attr("eclimit", 0)
                    .attr("expire", 0)
                    .attr("limit", 0)
                    .attr("status", 0),
            ),
        ),
        ("message.get", Node::new("response").child(Node::new("message").attr("status", 0))),
        ("facility.get", facility),
        ("pcbevent.put", Node::new("response").child(Node::new("pcbevent"))),
        ("package.list", Node::new("response").child(Node::new("package").attr("expire", 1200).attr("status", 0))),
        ("tax.get_phase", Node::new("response").child(Node::new("tax").child(s32("phase", 0)))),
        (
            "eventlog.write",
            Node::new("response").child(
                Node::new("eventlog")
                    .child(Node::value("gamesession", Value::S64(1)))
                    .child(s32("logsendflg", 0))
                    .child(s32("logerrlevel", 0))
                    .child(s32("evtidnosendflg", 0)),
            ),
        ),
        (
            "machine.get_control",
            Node::new("response").child(Node::new("machine").child(Node::new("command").child(Node::str("arg", "nop")))),
        ),
        // Steel Chronicle Victroopers
        ("KGG.system.getmaster", getmaster("MSwxLDEsMSwxLDEsMSwxLDEsMSwxLDEsMSwx")),
        (
            "KGG.hdkoperation.get",
            Node::new("response")
                .child(Node::new("hdkoperation").child(s32("nr_entry", 1)).child(Node::str("param", "0,0,0,0,0,0,0,0,0"))),
        ),
        // the Climax mode's records (`recordcount` u8, `record#<n>`: `recordid` u16, `playcount`
        // u16, `clear` u8): none
        ("KGG.climax.load", Node::new("response").child(Node::new("climax").child(Node::value("recordcount", Value::U8(0))))),
        // the SOUND VOLTEX collaboration: no SDVX play, no print
        (
            "KGG.soundvoltex.printcount",
            Node::new("response").child(Node::new("soundvoltex").child(s32("playcount", 0)).child(s32("printcount", 0))),
        ),
        ("KGG.soundvoltex.playcheck", Node::new("response").child(Node::new("soundvoltex").child(s32("playcheck", 0)))),
        // Metal Gear Arcade
        (
            "I36.system.getmaster",
            getmaster(
                "MjAxMTA4MTAwMDoxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MToxOjE6MQ==",
            ),
        ),
        ("empty", Node::new("response").child(Node::new("data"))),
    ];
    docs.into_iter().map(|(name, doc)| (name, doc.to_binary())).collect()
}
