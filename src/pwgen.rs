//! Strong, readable random passwords in the style of Apple's generator:
//! `bokdal-Tefmuz-9sanhr` — three pronounceable 6-letter groups, one capital,
//! one digit, joined by hyphens (20 characters, ≈ 74 bits of entropy).
//!
//! Randomness comes straight from the operating system's CSPRNG.

pub fn generate() -> String {
    let mut chars = syllables();
    let capital = capitalise_one(&mut chars);
    digit_for_one(&mut chars, capital);
    String::from_utf8(chars).expect("ASCII")
}

const GROUPS: usize = 3;
/// Consonant-vowel pattern of every 6-letter group: C V C C V C.
const PATTERN: [bool; 6] = [false, true, false, false, true, false];
const CONSONANTS: &[u8] = b"bcdfghjkmnprstvwz"; // no l, q, x, y: unambiguous and easy to type
const VOWELS: &[u8] = b"aeiou";
const DIGITS: &[u8] = b"23456789"; // no 0 or 1

/// `GROUPS` pronounceable groups joined by hyphens, all lowercase.
fn syllables() -> Vec<u8> {
    let group = || PATTERN.map(|vowel| pick(if vowel { VOWELS } else { CONSONANTS }));
    (0..GROUPS).map(|_| group()).collect::<Vec<_>>().join(&b'-')
}

/// Capitalise one letter and return its index. Never i or o, which look like
/// l and 0 when capitalised.
fn capitalise_one(chars: &mut [u8]) -> usize {
    let spots: Vec<usize> = (0..chars.len()).filter(|&i| !matches!(chars[i], b'-' | b'i' | b'o')).collect();
    let i = spots[below(spots.len())];
    chars[i] = chars[i].to_ascii_uppercase();
    i
}

/// Replace one letter other than `keep` with a digit.
fn digit_for_one(chars: &mut [u8], keep: usize) {
    let spots: Vec<usize> = (0..chars.len()).filter(|&i| chars[i] != b'-' && i != keep).collect();
    let i = spots[below(spots.len())];
    chars[i] = pick(DIGITS);
}

fn pick(set: &[u8]) -> u8 {
    set[below(set.len())]
}

/// Uniform random number in `0..n` (rejection sampling, no modulo bias).
fn below(n: usize) -> usize {
    assert!(n > 0 && n <= u32::MAX as usize);
    let n = n as u32;
    let zone = (u32::MAX / n) * n;
    loop {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).expect("the operating system's random source is unavailable");
        let v = u32::from_le_bytes(b);
        if v < zone {
            return (v % n) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn has_apple_style_shape() {
        for _ in 0..3000 {
            let p = generate();
            assert_eq!(p.len(), 20, "{p}");
            assert_eq!(p.matches('-').count(), 2, "{p}");
            assert_eq!(p.as_bytes()[6], b'-', "{p}");
            assert_eq!(p.as_bytes()[13], b'-', "{p}");
            assert_eq!(p.chars().filter(|c| c.is_ascii_uppercase()).count(), 1, "{p}");
            assert_eq!(p.chars().filter(|c| c.is_ascii_digit()).count(), 1, "{p}");
            assert!(
                p.chars().all(|c| c == '-' || c.is_ascii_alphanumeric()),
                "unexpected character in {p}"
            );
        }
    }

    #[test]
    fn avoids_look_alike_characters() {
        for _ in 0..3000 {
            let p = generate();
            for bad in ['l', 'L', 'I', 'O', '0', '1', 'q', 'y', 'x'] {
                assert!(!p.contains(bad), "{p} contains {bad}");
            }
        }
    }

    #[test]
    fn is_random() {
        let set: HashSet<String> = (0..2000).map(|_| generate()).collect();
        assert_eq!(set.len(), 2000);
        // every capital/digit position gets used (not stuck on one slot)
        let mut cap_pos = HashSet::new();
        for _ in 0..2000 {
            let p = generate();
            cap_pos.insert(p.find(|c: char| c.is_ascii_uppercase()).unwrap());
        }
        assert!(cap_pos.len() > 8);
    }

    /// `cargo test print_samples -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_samples() {
        for _ in 0..8 {
            println!("{}", generate());
        }
    }
}
