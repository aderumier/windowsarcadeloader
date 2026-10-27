//! A Banapassport card (Mifare, Aime format) for the Banapass reader (`bpreader.rs`): the
//! blocks and the Solitaire cipher ported from bananatools' `iccard/aime.c` and
//! `iccard/solitaire.c` (public domain).
//!
//! The card is its 20-digit access code: a 5-digit prefix the reader library accepts
//! (`01031`...), 8 digits (the card serial, Solitaire-encoded with the last 7 digits as the key),
//! 7 key digits. Bandai Namco's reader library (in Wangan Midnight Maximum Tune 5) makes the
//! code again from the serial and compares: the key digits come from the serial's MD5,
//! permuted by the prefix's 16 hex digits, folded into 23 bits ([`key7`]).
//! Its blocks: 0 the UID (the code's first 4 bytes), 1 "SBSD" and the serial (bytes 12..15, big
//! endian), 2 the access code as 10 BCD bytes (bytes 6..15).

const DECK_SIZE: usize = 22;
const JOKER_A: u8 = 21;
const JOKER_B: u8 = 22;

struct Deck([u8; DECK_SIZE]);

impl Deck {
    fn new(key: &[u8]) -> Self {
        let mut deck = Deck(std::array::from_fn(|i| i as u8 + 1));
        for &c in key {
            deck.hash();
            deck.cut(c - b'0' + 1);
        }
        deck
    }

    fn move_card(&mut self, card: u8) {
        let p = self.0.iter().position(|&c| c == card).unwrap_or(0);
        if p < DECK_SIZE - 1 {
            self.0.swap(p, p + 1);
        } else {
            for i in (2..DECK_SIZE).rev() {
                self.0[i] = self.0[i - 1];
            }
            self.0[1] = card;
        }
    }

    /// Moves the cards `0..point` after `point..DECK_SIZE - 1` (the last card stays).
    fn cut(&mut self, point: u8) {
        let point = point as usize;
        let mut tmp = self.0;
        tmp[..DECK_SIZE - point - 1].copy_from_slice(&self.0[point..DECK_SIZE - 1]);
        tmp[DECK_SIZE - point - 1..DECK_SIZE - 1].copy_from_slice(&self.0[..point]);
        self.0[..DECK_SIZE - 1].copy_from_slice(&tmp[..DECK_SIZE - 1]);
    }

    fn swap_outside_jokers(&mut self) {
        let mut jokers = self.0.iter().enumerate().filter(|(_, c)| **c == JOKER_A || **c == JOKER_B).map(|(i, _)| i);
        let (j1, j2) = (jokers.next().unwrap(), jokers.next().unwrap());
        let mut tmp = [0u8; DECK_SIZE];
        let mut n = 0;
        for &c in self.0[j2 + 1..].iter().chain(&self.0[j1..=j2]).chain(&self.0[..j1]) {
            tmp[n] = c;
            n += 1;
        }
        self.0 = tmp;
    }

    fn cut_by_bottom_card(&mut self) {
        let p = self.0[DECK_SIZE - 1];
        self.cut(if p == JOKER_B { JOKER_A } else { p });
    }

    fn top_card_num(&self) -> u8 {
        let p = self.0[0];
        self.0[if p == JOKER_B { JOKER_A } else { p } as usize]
    }

    fn hash(&mut self) {
        loop {
            self.move_card(JOKER_A);
            self.move_card(JOKER_B);
            self.move_card(JOKER_B);
            self.swap_outside_jokers();
            self.cut_by_bottom_card();
            let p = self.top_card_num();
            if p != JOKER_A && p != JOKER_B {
                break;
            }
        }
    }
}

/// Solitaire cipher of the digits `src` with the digit key `key`: encoding adds the stream,
/// decoding subtracts it (mod 10).
fn solitaire(key: &[u8], src: &[u8], decode: bool) -> Vec<u8> {
    let mut deck = Deck::new(key);
    src.iter()
        .map(|&c| {
            deck.hash();
            let p = deck.top_card_num() as i32;
            let v = (c - b'0') as i32 + if decode { -p } else { p };
            b'0' + v.rem_euclid(10) as u8
        })
        .collect()
}

/// The access code prefix of the cards made here and its key in the reader library's table.
const PREFIX: &str = "01031";
const PREFIX_KEY: &[u8; 16] = b"471B3E8629D0CA5F";

/// MD5 (RFC 1321).
fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_le_bytes());
    let mut h = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g]).rotate_left(S[i]));
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 16];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// The 7 key digits of a serial: its 8 digits' MD5, bytes permuted by the prefix key, the
/// 128-bit little endian number folded by XOR into 23 bits.
fn key7(serial: &str) -> String {
    let h = md5(serial.as_bytes());
    let mut p = [0u8; 16];
    for (i, c) in PREFIX_KEY.iter().enumerate() {
        p[i] = h[(*c as char).to_digit(16).unwrap() as usize];
    }
    let n = u128::from_le_bytes(p);
    let r = (0..6).fold(0u128, |r, k| r ^ ((n >> (23 * k)) & 0x7F_FFFF));
    format!("{:07}", r % 10_000_000)
}

pub(crate) struct Card {
    pub(crate) access_code: String,
    blocks: [[u8; 16]; 3],
}

impl Card {
    /// The card of an access code (20 digits, a known prefix), `None` otherwise.
    pub(crate) fn from_access_code(code: &str) -> Option<Card> {
        let digits = code.as_bytes();
        if digits.len() != 20 || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let bcd: Vec<u8> = digits.chunks(2).map(|d| (d[0] - b'0') << 4 | (d[1] - b'0')).collect();
        let serial = solitaire(&digits[13..], &digits[5..13], true);
        let serial: u32 = std::str::from_utf8(&serial).ok()?.parse().ok()?;
        let mut blocks = [[0u8; 16]; 3];
        blocks[0][..4].copy_from_slice(&bcd[..4]);
        blocks[1][..4].copy_from_slice(b"SBSD");
        blocks[1][12..].copy_from_slice(&serial.to_be_bytes());
        blocks[2][6..].copy_from_slice(&bcd);
        Some(Card { access_code: code.to_string(), blocks })
    }

    /// A new card: its serial from `seed`.
    pub(crate) fn generate(seed: u64) -> Card {
        let serial = format!("{:08}", seed % 100_000_000);
        let key = key7(&serial);
        let hashed = solitaire(key.as_bytes(), serial.as_bytes(), false);
        let code = format!("{PREFIX}{}{key}", String::from_utf8(hashed).unwrap());
        Card::from_access_code(&code).unwrap()
    }

    pub(crate) fn uid(&self) -> [u8; 4] {
        self.blocks[0][..4].try_into().unwrap()
    }

    pub(crate) fn block(&self, n: usize) -> [u8; 16] {
        self.blocks.get(n).copied().unwrap_or([0; 16])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solitaire_round_trip() {
        let enc = solitaire(b"1234567", b"00001234", false);
        assert_eq!(solitaire(b"1234567", &enc, true), b"00001234");
    }

    #[test]
    fn generated_card() {
        // the reader library's code for serial 12345678
        let card = Card::generate(12345678);
        assert_eq!(card.access_code, "01031519851353550221");
        assert_eq!(&card.block(1)[12..], &12345678u32.to_be_bytes());
    }

    #[test]
    fn md5_reference() {
        assert_eq!(md5(b"abc"), [0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1, 0x7f, 0x72]);
    }
}
