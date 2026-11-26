// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The text rules every person write follows (spec 2.2): how a name or value is cleaned before it is
//! stored, and the folded keys written beside it so a search, a sort and a duplicate check never
//! recompute anything at query time.
//!
//! All pure: no database, no clock. A change to a folding rule is a one-time backfill of the stored
//! keys, never a query-time recompute (spec 2.2), so these functions are the single owner of what a
//! key means.

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// What separates the words of a person's `search_key` from the phone-number digits that follow
/// them (ASCII "unit separator", which `clean_text` removes from every value, so no typed text can
/// forge it). The split lets a short word such as "12" match names and emails only, while a query
/// of three or more digits also reaches the phone numbers (spec 3.2).
pub const KEY_SPLIT: char = '\u{1f}';

/// The accent-insensitive key of a piece of text (E10): NFKD, drop only the Latin combining
/// diacritics (U+0300-U+036F), NFC, lowercase, collapse whitespace.
///
/// Marks of other scripts are kept on purpose: the Japanese voiced-sound mark (U+3099) and the
/// Devanagari vowel signs change the letter, so "ば" must never fold into "は". `pm_fold` (lowercase
/// only) cannot do this, which is why "elodie" never found "Élodie" before.
pub fn fold_key(text: &str) -> String {
    let stripped: String = text
        .nfkd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .nfc()
        .collect();
    let lowered = stripped.to_lowercase();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a character is one of the bidi and spoofing controls `clean_text` removes (spec 2.2):
/// U+061C, U+200E, U+200F, U+202A-U+202E, U+2066-U+2069, U+FEFF, U+2028 and U+2029.
fn is_spoofing(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Clean one single-line value: control characters (Cc) and the bidi and spoofing set are removed,
/// a tab or a line break becomes a space (so pasted "a<newline>b" stays two words), and the ends are
/// trimmed.
///
/// **Not `made_by::is_invisible`**, which also strips ZWNJ and ZWJ (U+200C, U+200D), variation
/// selectors and tag characters: Persian, Indic and emoji names need those, so they are kept here.
/// The same cleaning is applied to the Google side of every merge, so the two never differ by an
/// invisible character.
pub fn clean_text(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter_map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                Some(' ')
            } else if c.is_control() || is_spoofing(c) || c == KEY_SPLIT {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    cleaned.trim().to_string()
}

/// [`clean_text`] for the fields where a line break is real (notes, an address, an interaction's
/// note): line breaks survive, normalised to `\n`, a tab becomes a space (as in `clean_text`, so
/// pasted "Mon<tab>10am" keeps its words apart), and nothing else changes.
pub fn clean_multiline(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let cleaned: String = unified
        .chars()
        .filter_map(|c| {
            if c == '\t' {
                Some(' ')
            } else if c == '\n' || !(c.is_control() || is_spoofing(c)) {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    cleaned.trim().to_string()
}

/// First code point of each block of ten decimal digits (Unicode `Nd`) that NFKD does not already
/// map to ASCII. Fullwidth digits do fold; these scripts' own digits do not.
const DIGIT_BLOCKS: &[u32] = &[
    0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66,
    0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0,
];

/// The ASCII digit a decimal digit of any script stands for (`'٣'` is `'3'`), or `None` for any
/// other character.
pub fn ascii_digit(c: char) -> Option<char> {
    if c.is_ascii_digit() {
        return Some(c);
    }
    let cp = c as u32;
    DIGIT_BLOCKS
        .iter()
        .find(|&&start| (start..start + 10).contains(&cp))
        .and_then(|&start| char::from_digit(cp - start, 10))
}

/// A phone number's matching key: its digits, with a leading `+` kept (so "+44 20 7946 0000" and
/// "(020) 7946 0000" stay different numbers rather than guessing a country). Wide digits
/// ("１２３") count as ASCII through NFKD, and so do other scripts' own digits ("٠١٢"). No phone
/// library: matching is on this key alone (E10). A value with no digit has the empty key, which
/// must never be read as a match between two numbers.
pub fn digits_key(text: &str) -> String {
    let mut out = String::new();
    let mut seen_any = false;
    for c in text.nfkd() {
        if let Some(d) = ascii_digit(c) {
            out.push(d);
            seen_any = true;
        } else if c == '+' && !seen_any && out.is_empty() {
            out.push('+');
        }
    }
    // A lone "+" with nothing after it is no number.
    if out == "+" {
        out.clear();
    }
    out
}

/// An email address's matching key: trimmed and lowercased.
pub fn email_key(text: &str) -> String {
    text.trim().to_lowercase()
}

/// The key a list sorts a person by: their display name, folded, so "Élodie" sorts with "E" and
/// case never splits a run.
pub fn sort_key(display_name: &str) -> String {
    fold_key(display_name)
}

/// What an address contributes to `value_key`: the street, city and postal code, folded.
fn address_key(parts: Option<&Value>, value: &str) -> String {
    let pick = |name: &str| -> String {
        parts
            .and_then(|p| p.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let joined = [pick("street"), pick("city"), pick("postal_code")]
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        fold_key(value)
    } else {
        fold_key(&joined)
    }
}

/// The matching key of one detail (spec 2.2): an email is trimmed lowercase; a phone is its digits
/// with a leading `+` kept; an address is its folded street, city and postal code; work and school
/// are the folded organisation name; everything else is `fold_key(value)`.
pub fn value_key(kind: &str, value: &str, parts: Option<&Value>) -> String {
    match kind {
        "email" => email_key(value),
        "phone" => digits_key(value),
        "address" => address_key(parts, value),
        _ => fold_key(value),
    }
}

/// The names and details a person's `search_key` is made of. Borrowed, so a caller holding rows
/// from the database or a draft builds it without cloning.
pub struct SearchParts<'a> {
    pub display_name: &'a str,
    pub given_name: &'a str,
    pub middle_name: &'a str,
    pub family_name: &'a str,
    /// `(kind, value)` for every detail.
    pub details: &'a [(&'a str, &'a str)],
}

/// The folded haystack a search matches (spec 2.2): names, nicknames, former names, emails and
/// companies as words, then [`KEY_SPLIT`], then each phone's digits. Only the phone half answers a
/// query of three or more digits (see [`KEY_SPLIT`]).
pub fn search_key(parts: &SearchParts<'_>) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut digits: Vec<String> = Vec::new();
    let add = |into: &mut Vec<String>, s: String| {
        if !s.is_empty() && !into.contains(&s) {
            into.push(s);
        }
    };
    for name in [
        parts.display_name,
        parts.given_name,
        parts.middle_name,
        parts.family_name,
    ] {
        add(&mut words, fold_key(name));
    }
    for (kind, value) in parts.details {
        match *kind {
            "nickname" | "former_name" | "work" => add(&mut words, fold_key(value)),
            "email" => add(&mut words, fold_key(&email_key(value))),
            "phone" => add(&mut digits, digits_key(value)),
            _ => {}
        }
    }
    format!("{}{}{}", words.join(" "), KEY_SPLIT, digits.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_drops_latin_accents_case_and_extra_spaces() {
        assert_eq!(fold_key("  Élodie   MARTIN "), "elodie martin");
        assert_eq!(fold_key("Zoë"), "zoe");
    }

    #[test]
    fn folding_keeps_marks_of_other_scripts() {
        assert_ne!(fold_key("ば"), fold_key("は"));
        assert_ne!(fold_key("कु"), fold_key("क"));
    }

    #[test]
    fn clean_text_keeps_joiners_and_drops_spoofing_controls() {
        assert_eq!(clean_text("Sam\u{202e}Reed\u{200f}"), "SamReed");
        assert_eq!(clean_text("a\u{200c}b\u{200d}c"), "a\u{200c}b\u{200d}c");
        assert_eq!(clean_text("a\u{0007}b"), "ab");
        assert_eq!(clean_text("two\nlines"), "two lines");
        assert_eq!(clean_multiline("two\r\nlines\u{202e}"), "two\nlines");
    }

    #[test]
    fn a_tab_is_a_space_in_both_cleaners() {
        assert_eq!(clean_text("Mon\t10am"), "Mon 10am");
        assert_eq!(clean_multiline("Mon\t10am\nTue"), "Mon 10am\nTue");
        assert_eq!(clean_multiline("a\u{0007}b"), "ab");
    }

    #[test]
    fn other_scripts_digits_count_in_a_phone_key() {
        assert_eq!(digits_key("\u{660}\u{661}\u{662}\u{663}"), "0123");
        assert_eq!(digits_key("+\u{6f9}\u{6f8} 5"), "+985");
        assert_eq!(digits_key("\u{966}\u{967}\u{968}"), "012");
    }

    #[test]
    fn digits_key_keeps_only_a_leading_plus() {
        assert_eq!(digits_key("+44 (0)20 7946 0000"), "+4402079460000");
        assert_eq!(digits_key("020-7946-0000"), "02079460000");
        assert_eq!(digits_key("12+3"), "123");
        assert_eq!(digits_key("+"), "");
        assert_eq!(digits_key("\u{ff11}\u{ff12}\u{ff13}"), "123");
    }

    #[test]
    fn the_key_split_can_not_be_typed() {
        assert_eq!(clean_text("a\u{1f}b"), "ab");
        assert_eq!(clean_multiline("a\u{1f}b"), "ab");
    }

    #[test]
    fn an_address_key_is_street_city_and_postcode() {
        let parts = serde_json::json!({"street": "1 Example Road", "city": "Leeds", "postal_code": "LS1 4AB", "country": "UK"});
        assert_eq!(
            value_key("address", "ignored", Some(&parts)),
            "1 example road leeds ls1 4ab"
        );
        assert_eq!(value_key("address", "5 Á Lane", None), "5 a lane");
    }
}
