//! Human verification (architecture §7.6, §7.8): fingerprints, the six-emoji short authentication
//! string shown after an in-person QR scan, the 60-digit safety number for remote verification,
//! and the four-emoji call check.

use sha2::{Digest, Sha256, Sha512};

/// 64 emoji with English names (for VoiceOver and for reading aloud), indexed by 6 bits.
pub const EMOJI: [(&str, &str); 64] = [
    ("🐶", "Dog"),
    ("🐱", "Cat"),
    ("🦁", "Lion"),
    ("🐎", "Horse"),
    ("🦄", "Unicorn"),
    ("🐷", "Pig"),
    ("🐘", "Elephant"),
    ("🐰", "Rabbit"),
    ("🐼", "Panda"),
    ("🐓", "Rooster"),
    ("🐧", "Penguin"),
    ("🐢", "Turtle"),
    ("🐟", "Fish"),
    ("🐙", "Octopus"),
    ("🦋", "Butterfly"),
    ("🌷", "Flower"),
    ("🌳", "Tree"),
    ("🌵", "Cactus"),
    ("🍄", "Mushroom"),
    ("🌏", "Globe"),
    ("🌙", "Moon"),
    ("☁️", "Cloud"),
    ("🔥", "Fire"),
    ("🍌", "Banana"),
    ("🍎", "Apple"),
    ("🍓", "Strawberry"),
    ("🌽", "Corn"),
    ("🍕", "Pizza"),
    ("🎂", "Cake"),
    ("❤️", "Heart"),
    ("😀", "Smiley"),
    ("🤖", "Robot"),
    ("🎩", "Hat"),
    ("👓", "Glasses"),
    ("🔧", "Spanner"),
    ("🎅", "Santa"),
    ("👍", "Thumbs up"),
    ("☂️", "Umbrella"),
    ("⌛", "Hourglass"),
    ("⏰", "Clock"),
    ("🎁", "Gift"),
    ("💡", "Light bulb"),
    ("📕", "Book"),
    ("✏️", "Pencil"),
    ("📎", "Paperclip"),
    ("✂️", "Scissors"),
    ("🔒", "Lock"),
    ("🔑", "Key"),
    ("🔨", "Hammer"),
    ("☎️", "Telephone"),
    ("🏁", "Flag"),
    ("🚂", "Train"),
    ("🚲", "Bicycle"),
    ("✈️", "Aeroplane"),
    ("🚀", "Rocket"),
    ("🏆", "Trophy"),
    ("⚽", "Ball"),
    ("🎸", "Guitar"),
    ("🎺", "Trumpet"),
    ("🔔", "Bell"),
    ("⚓", "Anchor"),
    ("🎧", "Headphones"),
    ("📁", "Folder"),
    ("📌", "Pin"),
];

/// An emoji and its spoken name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiWord {
    /// The emoji glyph.
    pub emoji: String,
    /// Its English name.
    pub name: String,
}

/// The fingerprint of a device's MLS signature key (what the QR codes carry).
#[must_use]
pub fn fingerprint(signature_public_key: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"asiri-fingerprint-v1");
    h.update(signature_public_key);
    h.finalize().into()
}

/// Formats a fingerprint for display: 16 groups of 4 hex digits.
#[must_use]
pub fn format_fingerprint(fp: &[u8; 32]) -> String {
    hex::encode_upper(fp)
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn ordered<'a>(a: &'a [u8; 32], b: &'a [u8; 32]) -> (&'a [u8; 32], &'a [u8; 32]) {
    if a <= b { (a, b) } else { (b, a) }
}

fn emoji_from(hash: &[u8], count: usize) -> Vec<EmojiWord> {
    // Read `count` consecutive 6-bit groups, most significant bit first.
    let mut bits: u64 = 0;
    for b in &hash[..8] {
        bits = (bits << 8) | u64::from(*b);
    }
    (0..count)
        .map(|i| {
            let idx = ((bits >> (64 - 6 * (i + 1))) & 0x3f) as usize;
            let (e, n) = EMOJI[idx];
            EmojiWord {
                emoji: e.to_owned(),
                name: n.to_owned(),
            }
        })
        .collect()
}

/// The six emoji both screens show after an in-person scan. Symmetric in its arguments.
#[must_use]
pub fn pairing_emoji(fp_a: &[u8; 32], fp_b: &[u8; 32]) -> Vec<EmojiWord> {
    let (lo, hi) = ordered(fp_a, fp_b);
    let mut h = Sha256::new();
    h.update(b"asiri-sas-v1");
    h.update(lo);
    h.update(hi);
    emoji_from(&h.finalize(), 6)
}

/// The four emoji shown in the call window, derived from the call secret.
#[must_use]
pub fn call_emoji(call_secret: &[u8]) -> Vec<EmojiWord> {
    let mut h = Sha256::new();
    h.update(b"asiri-call-emoji-v1");
    h.update(call_secret);
    emoji_from(&h.finalize(), 4)
}

fn thirty_digits(fp: &[u8; 32]) -> String {
    let mut h = Sha512::new();
    h.update(b"asiri-safety-v1");
    h.update(fp);
    let d = h.finalize();
    d[..30]
        .chunks(5)
        .map(|c| {
            let v = c.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
            format!("{:05}", v % 100_000)
        })
        .collect()
}

/// The 60-digit safety number (12 groups of 5) two people compare over a call. Symmetric.
#[must_use]
pub fn safety_number(fp_a: &[u8; 32], fp_b: &[u8; 32]) -> String {
    let (lo, hi) = ordered(fp_a, fp_b);
    let digits = format!("{}{}", thirty_digits(lo), thirty_digits(hi));
    digits
        .as_bytes()
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_list_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for (e, n) in EMOJI {
            assert!(seen.insert(e), "duplicate emoji {e}");
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn verification_strings_are_symmetric_and_distinct() {
        let a = fingerprint(b"alice-key");
        let b = fingerprint(b"bob-key");
        let c = fingerprint(b"carol-key");
        assert_eq!(pairing_emoji(&a, &b), pairing_emoji(&b, &a));
        assert_eq!(pairing_emoji(&a, &b).len(), 6);
        assert_ne!(pairing_emoji(&a, &b), pairing_emoji(&a, &c));
        let sn = safety_number(&a, &b);
        assert_eq!(sn, safety_number(&b, &a));
        assert_eq!(sn.replace(' ', "").len(), 60);
        assert!(sn.replace(' ', "").bytes().all(|d| d.is_ascii_digit()));
        assert_eq!(call_emoji(b"secret").len(), 4);
        assert_eq!(format_fingerprint(&a).split(' ').count(), 16);
    }
}
