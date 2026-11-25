// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Made by X" on a task or milestone a mod made (Mods spec §1.5, §4; PR C, K5): the `mod_ledger` row
//! written in the same transaction as the task, joined onto the list the views read.
//!
//! **What the label is.** The mod's name as the ledger stored it when the thing was made (it survives
//! an uninstall: the row is outside the cascade), its tier, and whether the package was signed. It is
//! decoration, never a decision: nothing in PM trusts, permits or finds anything by it. The page draws
//! the name as clamped React text and ALWAYS with the tier word beside it, so an unsigned mod named
//! "Atlas" reads as unsigned.
//!
//! **A hostile ledger.** A shared vault's co-user can plant `mod_ledger` rows, so the name is cut to
//! [`NAME_MAX_CHARS`] and stripped of control characters here as well as clamped on the page, and a tier
//! outside the three is read as the least trusted (the table's CHECK holds that too).
//!
//! A task or milestone with no ledger row has no label (a LEFT JOIN: `NULL` columns). A ledger row of
//! a deleted task goes with it (`ON DELETE CASCADE`), and ids are never reused (AUTOINCREMENT), so a
//! label can never land on something another maker made.

use rusqlite::Row;
use serde::Serialize;

/// The most characters of a mod's name a label carries.
pub const NAME_MAX_CHARS: usize = 60;

/// Who made a task or milestone, as the ledger recorded it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MadeBy {
    pub name: String,
    /// `first`, `second` or `third`.
    pub tier: String,
    /// The package was signed. An unsigned maker is shown as such, whatever tier it claims.
    pub signed: bool,
}

/// Read the three columns a list query adds with `LEFT JOIN mod_ledger l ON l.task_id = t.id` (or
/// `l.milestone_id = m.id`) and `l.mod_name, l.tier, l.signed`, starting at `first`: `None` when
/// there is no ledger row.
pub fn read(r: &Row, first: usize) -> rusqlite::Result<Option<MadeBy>> {
    let name: Option<String> = r.get(first)?;
    let tier: Option<String> = r.get(first + 1)?;
    let signed: Option<bool> = r.get(first + 2)?;
    let (Some(name), Some(tier), Some(signed)) = (name, tier, signed) else {
        return Ok(None);
    };
    Ok(Some(clean(name, &tier, signed)))
}

/// Format and separator characters (Unicode Cf, Zl, Zp): invisible, and able to reorder or hide the
/// text around them. The page's rule for every mod-supplied name is `\p{Cc}\p{Cf}\p{Zl}\p{Zp}`
/// (`lib/modTextClean.ts`); std has no category lookup, so Cf is listed (soft hyphen, the Arabic
/// number and letter marks, the Mongolian vowel separator, zero-width and direction marks, the word
/// joiners and isolates, the BOM, the interlinear annotation marks, the musical and shorthand format
/// controls and the tag characters). Controls (Cc) go by `char::is_control`.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// A stored maker, made safe to draw: no control characters, at most [`NAME_MAX_CHARS`], and a tier
/// PM knows.
pub fn clean(name: String, tier: &str, signed: bool) -> MadeBy {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_invisible(*c))
        .take(NAME_MAX_CHARS)
        .collect();
    let tier = match tier {
        "first" | "second" | "third" => tier,
        _ => "third",
    };
    MadeBy {
        name: name.trim().to_owned(),
        tier: tier.to_owned(),
        signed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_planted_name_is_cut_and_stripped_and_an_odd_tier_reads_as_third_party() {
        let long = format!("{}\u{202e}\n{}", "a".repeat(40), "b".repeat(40));
        let made = clean(long, "root", false);
        assert!(made.name.chars().count() <= NAME_MAX_CHARS);
        assert!(!made.name.chars().any(|c| c.is_control() || is_invisible(c)));
        assert_eq!(made.tier, "third");
        assert!(!made.signed);
        assert_eq!(clean("Atlas".into(), "first", true).tier, "first");
    }

    #[test]
    fn the_format_and_separator_characters_the_page_also_cleans_are_gone() {
        for c in [
            '\u{061C}',
            '\u{00AD}',
            '\u{180E}',
            '\u{2028}',
            '\u{2029}',
            '\u{206A}',
            '\u{E0041}',
            '\u{E007F}',
        ] {
            let made = clean(format!("At{c}las"), "third", false);
            assert_eq!(made.name, "Atlas", "{c:?}");
        }
        assert_eq!(clean("Ünïcode ✓".into(), "third", false).name, "Ünïcode ✓");
    }
}
