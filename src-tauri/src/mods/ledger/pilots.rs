// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ledger's half of the pilots' auto-install (`mods::autoinstall`, which holds the table, the
//! decision and the handover's SQL): reading what each marker needs decided, installing a downloaded
//! pilot unconsented with its marker and handover in the row's own transaction, and settling a pilot
//! that already has a row. Nothing here downloads, and nothing here is reached with the DB lock held.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::{
    Approval, InstallOptions, Installed, Ledger, LedgerError, LedgerStore, RepairTarget, Source,
};
use crate::mods::autoinstall::{self, Held, Pilot, Step};
use crate::mods::index::Index;
use crate::mods::keys::TrustedKeys;
use crate::mods::policy::{InstallKind, Refusal};
use crate::mods::safety::{Mode, SafetyCheck};
use crate::mods::store;
use crate::mods::updater::Download;

/// What a downloaded pilot came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PilotLanded {
    Installed(Installed),
    /// Refused or failed; the text is for the log. Nothing changed: the marker stays, so the next
    /// pass tries again.
    Failed(String),
}

/// The pilot's row as the decision reads it: where it came from, whether it is signed, and whether
/// it is parked for want of its files.
fn held(conn: &Connection, id: &str) -> rusqlite::Result<Option<(String, bool, bool)>> {
    conn.query_row(
        "SELECT installed_from, signed, state = ?2 AND state_reason = ?3 FROM mods WHERE id = ?1",
        params![id, store::STATE_PARKED, store::REASON_FILES_MISSING],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Option<bool>>(2)?.unwrap_or(false),
            ))
        },
    )
    .optional()
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// What each marker present is owed against `index` at `now` (pure over the rows read).
    pub fn pilot_plan(
        &self,
        index: Option<&Index>,
        now: DateTime<Utc>,
    ) -> Result<Vec<(&'static Pilot, Step)>, LedgerError> {
        self.with_conn(|c| {
            let mut out = Vec::new();
            for pilot in autoinstall::pending(c)? {
                let row = held(c, pilot.id)?;
                let held = row.as_ref().map(|(from, signed, files_missing)| Held {
                    installed_from: from,
                    signed: *signed,
                    files_missing: *files_missing,
                });
                out.push((pilot, autoinstall::decide(pilot, held, index, now)));
            }
            Ok(out)
        })
    }

    /// A pilot that already has a catalogue row: its handover and the marker's removal in one
    /// transaction, re-checking the row there. Writes nothing when the row is no longer one.
    pub fn settle_pilot(&self, pilot: &Pilot) -> Result<(), LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        self.with_conn(|c| {
            let tx = c.transaction()?;
            let settles = held(&tx, pilot.id)?
                .is_some_and(|(from, signed, _)| from == store::FROM_CATALOGUE && signed);
            if !settles {
                return Err(Refusal::Changed.into());
            }
            autoinstall::finish(&tx, pilot)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// A pilot whose row is here and whose files aren't (a restore onto another computer): put back
    /// the pinned version and file downloaded for `target` (the ledger's own repair, which accepts
    /// nothing else), then settle, which clears the marker. The settle runs only when the repair
    /// did; a settle that fails leaves the marker, and the next pass settles a row that is whole.
    pub fn repair_pilot(
        &self,
        pilot: &Pilot,
        target: &RepairTarget,
        bytes: &[u8],
        keys: &TrustedKeys,
        index: Option<&Index>,
        now: DateTime<Utc>,
    ) -> Result<(), LedgerError> {
        self.install_repair(target, bytes, keys, index, now)?;
        self.settle_pilot(pilot)
    }

    /// Install a package the pass downloaded for `pilot`, as a fresh unconsented install of a
    /// catalogue mod: the key-id gate, the verifier, the catalogue check against `index` (the file
    /// is the listed one, the blacklist doesn't name it) and the ledger's own checks all run. The
    /// marker and the handover run in the row's transaction; an install that isn't a fresh one (the
    /// mod appeared since the plan) is rolled back, never an update that nobody approved.
    pub fn land_pilot(
        &self,
        pilot: &Pilot,
        download: &Download,
        bytes: &[u8],
        keys: &TrustedKeys,
        index: &Index,
        now: DateTime<Utc>,
    ) -> PilotLanded {
        if download.id != pilot.id {
            return PilotLanded::Failed("that download isn't for this pilot".to_owned());
        }
        let check = SafetyCheck::catalogue(Some(index), now, Mode::Latest);
        let opts = InstallOptions {
            source: Source::Catalogue,
            // D17: the first-open screen is where the user turns it on and answers for its grants.
            consented: false,
            // A fresh install has no row to widen, and nothing is granted by it.
            approval: Approval::Approved,
            catalogue: &check,
        };
        let result = self.install_package_with(bytes, keys, &opts, now, &|c, kind| {
            if kind != InstallKind::Fresh {
                return Err(Refusal::Changed.into());
            }
            autoinstall::finish(c, pilot)
        });
        match result {
            Ok(installed) => PilotLanded::Installed(installed),
            Err(e) => PilotLanded::Failed(e.to_string()),
        }
    }
}
