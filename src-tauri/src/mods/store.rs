// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The closed value sets of the v60 mod tables (spec §1.12), one owner each (INVARIANTS.md I-12).
//! Every writer takes its strings from here, and a pin test in `db/migrations.rs` reads each set
//! back out of v60's CHECKs, so the two can't drift. `mods.tier` is owned by [`super::ids::Tier`].
//!
//! The ledger (`ledger.rs`: install, update, uninstall, the startup load), the Mods tab's commands
//! and the dispatcher are the writers. The folder test-mod loader (slice 6, part 4) is the one
//! writer still to come: `FROM_FOLDER` and the pieces only it uses carry their own allowances.

use super::package::DataClass;

/// `mods.state`. `installing` until the files are written and verified; `parked` makes no calls
/// (a closed room, missing files, an API major outside the window); `paused` is the user's, the
/// blacklist's or dev mode's (spec §1.3, §1.10, §1.15). The whole set is read by the pin tests
/// (here and in `db/migrations.rs`).
#[cfg_attr(not(test), allow(dead_code))]
pub const STATES: [&str; 4] = [STATE_ACTIVE, STATE_PAUSED, STATE_PARKED, STATE_INSTALLING];
pub const STATE_ACTIVE: &str = "active";
pub const STATE_PAUSED: &str = "paused";
pub const STATE_PARKED: &str = "parked";
pub const STATE_INSTALLING: &str = "installing";

/// `mods.state_reason` for the parks the ledger itself makes and lifts (`mods/ledger.rs`). Free
/// text in the schema; other owners (a closed room, the blacklist, dev mode) bring their own words
/// and the ledger leaves those rows alone.
pub const REASON_FILES_MISSING: &str = "files-missing";
/// A file is there but isn't what was installed: another size, or not a plain file.
pub const REASON_FILES_CHANGED: &str = "files-changed";
/// The row itself can't be read back into a mod (its id, version, file list or manifest).
pub const REASON_RECORD: &str = "record-unreadable";
/// Its API range is outside what this PM serves (a newer API, or a major past the window).
pub const REASON_API: &str = "api-unsupported";
pub const LEDGER_REASONS: [&str; 4] = [
    REASON_FILES_MISSING,
    REASON_FILES_CHANGED,
    REASON_RECORD,
    REASON_API,
];

pub fn is_ledger_reason(reason: &str) -> bool {
    LEDGER_REASONS.contains(&reason)
}

/// `mods.state_reason` of a mod the user paused from the Mods tab. Only the user's own resume lifts
/// it: the blacklist leaves such a row alone ([`super::safety::plan_row`]) and the ledger keeps it
/// through an update ([`super::policy::state_after_install`]).
pub const REASON_USER: &str = "user";
/// `mods.state_reason` of an unsigned mod paused because developer mode is off (spec §1.15), until
/// it is on again. Written by the ledger's load (`policy::dev_mode_state`), which pauses an active
/// unsigned row while developer mode is off for this device and resumes exactly these rows when it
/// is on; the Mods tab only words it.
pub const REASON_DEV_MODE: &str = "dev-mode";

/// `mods.state_reason` of a mod the signed index paused (Mods spec §1.10): stopped, its data kept.
/// The index's own words are read from the cached index, not stored here, so a corrected reason
/// reaches the strip.
pub const REASON_BLACKLIST: &str = "blacklist";
/// `mods.state_reason` of a mod the index removed: switched off, its grants revoked, its files and
/// cache-class rows deleted. The row stays, in `paused`, until the user answers the uninstall
/// dialog about what it made and the content it stored (I-16, I-31), so this is the typed
/// "pending removal" state slice 6 reads. No install or update lands on it, and the startup load
/// never looks for its files.
pub const REASON_BLACKLIST_REMOVED: &str = "blacklist-removed";

/// `settings` key: the mods the blacklist resumed, awaiting slice 6's "X is back on, fixed in
/// version N" strip. A JSON array of `{id, version, at}`, one per mod, newest last; the strip's
/// dismissal removes its entry.
pub const SETTING_RESUMED: &str = "mods_resumed";
/// `settings` key: background updates that wait for approval (they ask for more than was
/// granted, or need a newer PM), a JSON object keyed by mod id, each value a
/// [`super::ledger::Waiting`]: `{version, sha256, kind, widenings, needsApi}` (camelCase; `kind` is
/// `approval` or `newer-pm`, which decides whether the strip asks for approval or says to update
/// PM). Slice 6's "update waiting" reads it; an approved install or a newer update clears it.
pub const SETTING_UPDATES_WAITING: &str = "mods_updates_waiting";

/// `settings` key: which devices have accepted the Mods disclaimer in this vault, a JSON object
/// `{"v": 1, "devices": ["<device id>", ...]}` (`tab::disclaimer`). The vault-side half of "per
/// device AND per vault": a new machine, or a vault that has never heard of this one, asks again.
pub const SETTING_DISCLAIMER: &str = "mods_disclaimer";

/// `settings` key: which devices have developer mode on in this vault, a JSON object
/// `{"v": 1, "devices": ["<device id>", ...]}` (`mods::devmode`). Developer mode is per device AND
/// per vault, so a restore onto another machine or a joined shared vault never inherits it, and
/// switching it never reaches another machine using the same vault. Not in the webview's pref
/// allow-list: only Rust reads or writes it, and unsigned installs trust nothing else.
pub const SETTING_DEV_DEVICES: &str = "mods_dev_devices";
/// The pre-per-device boolean row (`"true"`/`"false"`), once webview-written. Read once, to carry an
/// old "on" over to the device that first reads it; never an authority after that
/// ([`SETTING_DEV_DEVICES`] exists), and a restore turns it off too.
pub const SETTING_DEV_MODE_LEGACY: &str = "dev_mode";

/// `mods.installed_from`: the signed catalogue, a `.pmmod` file (the Mods tab's "Install from
/// file"), or a developer's unpacked test folder, which has no package sha256 and is left out of
/// backups (§1.15). The whole set is read by the pin tests.
#[cfg_attr(not(test), allow(dead_code))]
pub const INSTALLED_FROM: [&str; 3] = [FROM_CATALOGUE, FROM_FILE, FROM_FOLDER];
pub const FROM_CATALOGUE: &str = "catalogue";
pub const FROM_FILE: &str = "file";
/// Written by the folder test-mod loader (slice 6, part 4), which doesn't exist yet.
#[cfg_attr(not(test), allow(dead_code))]
pub const FROM_FOLDER: &str = "folder";

/// `mod_grants.decision`. No row means the permission hasn't been asked yet (a first-use grant).
/// The whole set is read by the pin tests.
#[cfg_attr(not(test), allow(dead_code))]
pub const DECISIONS: [&str; 2] = [GRANTED, DENIED];
pub const GRANTED: &str = "granted";
pub const DENIED: &str = "denied";

/// `mod_kv.class`: the manifest's class for the key. Content is the user's (kept on update, asked
/// about on uninstall, in backups by default, counted as user data); cache is rebuildable and
/// stripped from backups (D4). The whole set is read by the pin tests.
#[cfg_attr(not(test), allow(dead_code))]
pub const KV_CLASSES: [&str; 2] = [KV_CONTENT, KV_CACHE];
pub const KV_CONTENT: &str = "content";
pub const KV_CACHE: &str = "cache";

/// The stored class for a manifest data key's class.
pub fn kv_class(class: DataClass) -> &'static str {
    match class {
        DataClass::Content => KV_CONTENT,
        DataClass::Cache => KV_CACHE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_class_is_the_manifests_word_for_it() {
        // The manifest's `data.keys[].class` and the stored `mod_kv.class` must be one vocabulary,
        // or a key the manifest calls content would be stored under a word the backup strips.
        for word in KV_CLASSES {
            let class: DataClass = serde_json::from_value(serde_json::json!(word)).unwrap();
            assert_eq!(kv_class(class), word);
        }
    }

    #[test]
    fn every_set_is_its_named_values_without_repeats() {
        for set in [
            &STATES[..],
            &INSTALLED_FROM,
            &DECISIONS,
            &KV_CLASSES,
            &LEDGER_REASONS,
        ] {
            let mut seen = std::collections::BTreeSet::new();
            assert!(
                set.iter().all(|v| seen.insert(*v)),
                "{set:?} repeats a value"
            );
        }
    }
}
