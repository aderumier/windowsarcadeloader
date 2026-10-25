//! The e-Amusement card manager (`cardmng`): cards and their PIN, kept in
//! `dev/nvram/easrv/cards.txt` (`<card id> <refid> <PIN>` per line). A card registers on the
//! game's screens (new PIN, entered twice), or beforehand: `WAL_KONAMI_CARD_PIN` registers the
//! card of `WAL_KONAMI_CARD_ID` (`preregister`).
//!
//! - `inquire` (`cardid`): an unknown card answers status 112 (the game offers to register it);
//!   a known one its refid (`newflag` 1 until the game bound it with `bindmodel`).
//! - `getrefid` (`cardid`, `passwd`): registers the card with that PIN.
//! - `authpass` (`refid`, `pass`): 0 the PIN is right, 116 wrong.
//! - `bindmodel` (`refid`): the card used with this game (its data exists).

use std::sync::Mutex;

use crate::kbin::Node;

pub const DIR: &str = "dev/nvram/easrv";
const CARDS: &str = "dev/nvram/easrv/cards.txt";

struct Card {
    id: String,
    refid: String,
    pin: String,
}

static LOCK: Mutex<()> = Mutex::new(());

fn load() -> Vec<Card> {
    let text = std::fs::read_to_string(CARDS).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some(Card { id: it.next()?.to_string(), refid: it.next()?.to_string(), pin: it.next().unwrap_or_default().to_string() })
        })
        .collect()
}

fn save(cards: &[Card]) {
    let _ = std::fs::create_dir_all(DIR);
    let text: String = cards.iter().map(|c| format!("{} {} {}\n", c.id, c.refid, c.pin)).collect();
    let _ = std::fs::write(CARDS, text);
}

/// A card's refid: 16 hex digits made from its id (stable, unique per card).
fn refid_of(card: &str) -> String {
    let h = crate::kbin::md5(card.to_ascii_uppercase().as_bytes());
    h[..8].iter().map(|b| format!("{b:02X}")).collect()
}

/// Whether the game keeps data for this card (a `bindmodel`, or player data saved).
fn bound(refid: &str) -> bool {
    std::path::Path::new(&format!("{DIR}/{refid}")).exists()
}

/// The card of `WAL_KONAMI_CARD_ID` (16 hex digits; else the payload's default card, as its
/// card reader reads it).
fn card_id() -> String {
    let id = std::env::var("WAL_KONAMI_CARD_ID").map(|v| v.trim().to_ascii_uppercase()).unwrap_or_default();
    if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) { id } else { "E004010027104B1D".into() }
}

/// The card of `WAL_KONAMI_CARD_ID` registered with `WAL_KONAMI_CARD_PIN` (4 digits), when
/// set: the game then knows the card (no registration on its screen) and asks that PIN.
pub fn preregister() {
    let Ok(pin) = std::env::var("WAL_KONAMI_CARD_PIN").map(|p| p.trim().to_string()) else { return };
    if pin.is_empty() {
        return;
    }
    if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
        crate::log!("easrv: WAL_KONAMI_CARD_PIN must be 4 digits, the card is not registered");
        return;
    }
    let id = card_id();
    let _guard = LOCK.lock().unwrap();
    let mut cards = load();
    match cards.iter_mut().find(|c| c.id.eq_ignore_ascii_case(&id)) {
        Some(c) if c.pin == pin => return,
        Some(c) => c.pin = pin,
        None => cards.push(Card { refid: refid_of(&id), id: id.clone(), pin }),
    }
    save(&cards);
    crate::log!("easrv: card {id} registered (PIN from WAL_KONAMI_CARD_PIN)");
}

/// `system.convcardnumber` (`data/card_id`): the card's printed number (16 characters; here its
/// id, the printed one is an encoding of it the game only shows).
pub fn convcardnumber(req: Option<&Node>) -> Node {
    let id = req.and_then(|r| r.path("system/data/card_id")).map(|n| n.text()).unwrap_or_default();
    Node::new("system")
        .child(Node::new("data").child(Node::str("card_number", &id.to_ascii_uppercase())))
        .child(Node::value("result", crate::kbin::Value::S32(0)))
}

/// `group.getid` (`group/dataid`): the player's group (team): none (arkkgg.dll reads `dataid`,
/// `groupid` s32, `date`).
pub fn group_getid(req: Option<&Node>) -> Node {
    let dataid = req.and_then(|r| r.path("group/dataid")).map(|n| n.text()).unwrap_or_default();
    Node::new("group")
        .child(Node::str("dataid", &dataid))
        .child(Node::value("groupid", crate::kbin::Value::S32(0)))
        .child(Node::str("date", "2016-01-01 00:00:00"))
}

/// The `cardmng.<method>` answer (the `<response>`'s child).
pub fn handle(method: &str, req: Option<&Node>) -> Node {
    let _guard = LOCK.lock().unwrap();
    let req = req.and_then(|r| r.find("cardmng"));
    let attr = |n: &str| req.and_then(|r| r.get_attr(n)).unwrap_or_default().to_string();
    let mut cards = load();
    match method {
        "inquire" => {
            let id = attr("cardid");
            match cards.iter().find(|c| c.id.eq_ignore_ascii_case(&id)) {
                None => Node::new("cardmng").attr("status", 112),
                Some(c) => {
                    let bound = bound(&c.refid);
                    Node::new("cardmng")
                        .attr("refid", &c.refid)
                        .attr("dataid", &c.refid)
                        .attr("newflag", if bound { 0 } else { 1 })
                        .attr("binded", if bound { 1 } else { 0 })
                        .attr("expired", 0)
                        .attr("ecflag", 1)
                        .attr("useridflag", 1)
                        .attr("extidflag", 1)
                        .attr("status", 0)
                }
            }
        }
        "getrefid" => {
            let id = attr("cardid");
            let refid = refid_of(&id);
            cards.retain(|c| !c.id.eq_ignore_ascii_case(&id));
            cards.push(Card { id, refid: refid.clone(), pin: attr("passwd") });
            save(&cards);
            Node::new("cardmng").attr("refid", &refid).attr("dataid", &refid).attr("pcode", "").attr("status", 0)
        }
        "authpass" => {
            let refid = attr("refid");
            let ok = cards.iter().any(|c| c.refid == refid && (c.pin.is_empty() || c.pin == attr("pass")));
            Node::new("cardmng").attr("status", if ok { 0 } else { 116 })
        }
        "bindmodel" => {
            let refid = attr("refid");
            let _ = std::fs::create_dir_all(format!("{DIR}/{refid}"));
            Node::new("cardmng").attr("dataid", &refid).attr("status", 0)
        }
        _ => Node::new("cardmng").attr("status", 0),
    }
}
