// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Mods tab's command surface for signed mods (Mods spec §1.13, D17; slice 6, part 2). The page
//! (built by the UI part) draws these answers and decides nothing: what a row says, what each
//! permission means now, what an install will do, what an uninstall takes and leaves, and every
//! refusal come from here, in words.
//!
//! - [`listing`]: `list_mods`, one DTO per installed row from the row and its stored manifest.
//! - [`words`]: the standing of a row as a typed kind, a sentence and the one fix on offer.
//! - [`icon`]: `mod_icon`, a PNG or WebP of the verified files, as bytes for a `blob:` URL.
//! - [`install`], [`stage`], [`consent`]: a signed install in two steps with the user's answers
//!   between them: the consent screen first, nothing installed; then the staged bytes, installed,
//!   and the answers written as `mod_grants` rows.
//! - [`catalogue`]: the Available section, from the cached signed index.
//! - [`uninstall`]: the preview, the uninstall and, only when asked, deleting what the mod made.
//! - [`disclaimer`], [`device`]: the Mods disclaimer, per device AND per vault.
//! - [`status`]: how fresh the safety list is, the record's health and the strips.
//! - [`error`]: the typed refusals.
//! - [`commands`]: the Tauri commands, over all of it.
//!
//! The user's own pause, resume and first-open "Turn on" are the ledger's (`ledger/user.rs`): they
//! change rows and the registry, so they live beside the other writers of both.

pub mod catalogue;
pub mod commands;
pub mod consent;
pub mod device;
pub mod disclaimer;
pub mod error;
pub mod icon;
pub mod install;
pub mod listing;
pub mod stage;
pub mod status;
#[cfg(test)]
pub(crate) mod testing;
pub mod uninstall;
pub mod words;
