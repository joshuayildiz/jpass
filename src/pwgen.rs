//! Strong, readable random passwords in the style of Apple's generator:
//! `bokdal-Tefmuz-9sanhr` — three pronounceable 6-letter groups, one capital,
//! one digit, joined by hyphens (20 characters, ≈ 74 bits of entropy).
//!
//! Randomness comes straight from the operating system's CSPRNG.

const CONSONANTS: &[u8] = b"bcdfghjkmnprstvwz"; // no l, q, x, y: unambiguous and easy to type
const VOWELS: &[u8] = b"aeiou";
const DIGITS: &[u8] = b"23456789"; // no 0 or 1
/// Consonant-vowel pattern of every 6-letter group: C V C C V C.
const PATTERN: [bool; 6] = [false, true, false, false, true, false]; // true = vowel
const GROUPS: usize = 3;

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

fn pick(set: &[u8]) -> u8 {
    set[below(set.len())]
}

pub fn generate() -> String {
    let mut chars: Vec<u8> = Vec::with_capacity(GROUPS * 7);
    for g in 0..GROUPS {
        if g > 0 {
            chars.push(b'-');
        }
        for &vowel in &PATTERN {
            chars.push(pick(if vowel { VOWELS } else { CONSONANTS }));
        }
    }

    // One capital letter (never i or o, which look like l and 0 when capitalised).
    let letters: Vec<usize> = (0..chars.len()).filter(|&i| chars[i] != b'-').collect();
    let capital_spots: Vec<usize> =
        letters.iter().copied().filter(|&i| !matches!(chars[i], b'i' | b'o')).collect();
    let cap = capital_spots[below(capital_spots.len())];
    chars[cap] = chars[cap].to_ascii_uppercase();

    // One digit replacing a different letter.
    let digit_spots: Vec<usize> = letters.into_iter().filter(|&i| i != cap).collect();
    let dig = digit_spots[below(digit_spots.len())];
    chars[dig] = pick(DIGITS);

    String::from_utf8(chars).expect("ASCII")
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
