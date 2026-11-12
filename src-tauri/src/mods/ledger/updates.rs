// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ledger's half of the background updater and of a pinned-version repair (the decisions and
//! the download are `mods::updater`, the schedule `mods::refresh`): reading the rows the updater
//! plans over, installing a downloaded package as an *unattended* update, and recording an update
//! that has to wait for the user. Nothing here downloads, and nothing here is reached with the DB
//! lock held.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

use super::blacklist::{self, Waiting, WaitingKind};
use super::{Approval, InstallOptions, Installed, Ledger, LedgerError, LedgerStore, Source};
use crate::mods::ids::ModId;
use crate::mods::index::Index;
use crate::mods::keys::TrustedKeys;
use crate::mods::policy::{Refusal, Widening};
use crate::mods::safety::{ModNotice, Mode, SafetyCheck};
use crate::mods::updater::{self, Download, Step, UpdateRow};

/// What a downloaded update came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landed {
    Installed(Installed),
    /// It asks for more than was granted (or needs a newer PM) and is recorded as waiting.
    /// `changed`: this is news (no identical record was there), so the page is told.
    Waiting {
        waiting: Waiting,
        changed: bool,
    },
    /// Refused or failed; the text is for the log. The next accepted fetch tries again.
    Failed(String),
}

/// What one planning pass over the installed rows decided.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
    /// Packages to download and offer to [`Ledger::land`].
    pub downloads: Vec<Download>,
    /// News for the page: updates that need a newer PM, recorded just now.
    pub notices: Vec<ModNotice>,
}

/// What a repair puts back: the pinned version and file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairTarget {
    pub url: String,
    pub version: String,
    pub sha256: String,
}

fn read_update_rows(conn: &Connection) -> rusqlite::Result<Vec<UpdateRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, version, tier, author, source_repo, installed_from, state_reason \
         FROM mods ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(UpdateRow {
            id: r.get(0)?,
            version: r.get(1)?,
            tier: r.get(2)?,
            author: r.get(3)?,
            source_repo: r.get(4)?,
            installed_from: r.get(5)?,
            reason: r.get(6)?,
        })
    })?;
    rows.collect()
}

fn widening_text(w: &Widening) -> String {
    match w {
        Widening::Permission(p) => format!("permission:{p}"),
        Widening::Host(h) => format!("host:{h}"),
        Widening::Secret(s) => format!("secret:{s}"),
        Widening::Contribution(c) => format!("contribution:{c}"),
    }
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// What the updater would do for each installed row against `index` at `now`.
    pub fn plan_updates(
        &self,
        index: &Index,
        now: DateTime<Utc>,
    ) -> Result<Vec<(String, Step)>, LedgerError> {
        let (rows, waiting) =
            self.with_conn(|c| Ok((read_update_rows(c)?, blacklist::read_waiting(c)?)))?;
        Ok(updater::plan(&rows, &waiting, index, now))
    }

    /// The updater's planning, with its side effects: the downloads to make, and an update that
    /// needs a newer PM recorded as waiting (and announced once, when it is news).
    pub fn plan_pass(&self, index: &Index, now: DateTime<Utc>) -> Result<Pass, LedgerError> {
        let mut pass = Pass::default();
        for (id, step) in self.plan_updates(index, now)? {
            match step {
                Step::Download(d) => pass.downloads.push(d),
                Step::NeedsNewerPm {
                    version,
                    needs_api,
                    sha256,
                } => match self.note_needs_newer_pm(&id, &version, &needs_api, &sha256) {
                    Ok(Landed::Waiting { changed: true, .. }) => {
                        pass.notices.push(ModNotice::UpdateNeedsNewerPm {
                            name: self.name_of(&id),
                            id,
                            version,
                            needs_api,
                        });
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("mods: {id}: the update wasn't recorded ({e})"),
                },
                Step::Skip(_) => {}
            }
        }
        Ok(pass)
    }

    /// [`Ledger::install_download`], plus the notice an update that now waits for the user is
    /// owed (once: only when the record is new).
    pub fn land(
        &self,
        download: &Download,
        bytes: &[u8],
        keys: &TrustedKeys,
        index: &Index,
        now: DateTime<Utc>,
    ) -> (Landed, Vec<ModNotice>) {
        let landed = self.install_download(download, bytes, keys, index, now);
        let mut notices = Vec::new();
        if let Landed::Waiting {
            waiting,
            changed: true,
        } = &landed
        {
            notices.push(ModNotice::UpdateWaiting {
                id: download.id.clone(),
                name: self.name_of(&download.id),
                version: waiting.version.clone(),
            });
        }
        (landed, notices)
    }

    /// A mod's display name for a notice, or its id when it can't be read.
    fn name_of(&self, id: &str) -> String {
        self.with_conn(|c| {
            Ok(
                c.query_row("SELECT name FROM mods WHERE id = ?1", [id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?,
            )
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| id.to_owned())
    }

    /// Record that `id`'s newest version needs a newer PM, once: an identical record already there
    /// is not news.
    pub fn note_needs_newer_pm(
        &self,
        id: &str,
        version: &str,
        needs_api: &str,
        sha256: &str,
    ) -> Result<Landed, LedgerError> {
        let waiting = Waiting {
            version: version.to_owned(),
            sha256: sha256.to_owned(),
            kind: WaitingKind::NewerPm,
            widenings: Vec::new(),
            needs_api: Some(needs_api.to_owned()),
        };
        self.record_waiting(id, waiting)
    }

    fn record_waiting(&self, id: &str, waiting: Waiting) -> Result<Landed, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let changed = self.with_conn(|c| {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM mods WHERE id = ?1)",
                [id],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(LedgerError::NotInstalled);
            }
            let same = blacklist::read_waiting(c)?.get(id) == Some(&waiting);
            if !same {
                blacklist::set_waiting(c, id, waiting.clone())?;
            }
            Ok(!same)
        })?;
        Ok(Landed::Waiting { waiting, changed })
    }

    /// Install a package the updater downloaded, as an unattended update of the catalogue mod
    /// `download` names: the key-id gate, the verifier, the catalogue check against `index` and the
    /// ledger's own checks all run. An update that asks for more than was granted is recorded as
    /// waiting, not applied.
    pub fn install_download(
        &self,
        download: &Download,
        bytes: &[u8],
        keys: &TrustedKeys,
        index: &Index,
        now: DateTime<Utc>,
    ) -> Landed {
        let check = SafetyCheck::catalogue(Some(index), now, Mode::Latest);
        let opts = InstallOptions {
            source: Source::Catalogue,
            // An update keeps the row's own consent; it never gives any.
            consented: false,
            approval: Approval::Unattended,
            catalogue: &check,
        };
        match self.install_package(bytes, keys, &opts, now) {
            Ok(installed) => Landed::Installed(installed),
            Err(LedgerError::Refused(Refusal::NeedsApproval(wider))) => {
                let waiting = Waiting {
                    version: download.version.clone(),
                    sha256: download.sha256.clone(),
                    kind: WaitingKind::Approval,
                    widenings: wider.iter().map(widening_text).collect(),
                    needs_api: None,
                };
                self.record_waiting(&download.id, waiting)
                    .unwrap_or_else(|e| Landed::Failed(e.to_string()))
            }
            Err(e) => Landed::Failed(e.to_string()),
        }
    }

    /// The pinned version and file of an installed, signed mod, and the URL it is re-fetched from,
    /// for a repair. Refused for a row the safety list removed, and for one with no pinned file.
    pub fn repair_target(&self, id: &ModId) -> Result<RepairTarget, LedgerError> {
        let (version, sha256, signed, reason) = self
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT version, package_sha256, signed, state_reason FROM mods WHERE id = ?1",
                    [id.as_str()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, bool>(2)?,
                            r.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?)
            })?
            .ok_or(LedgerError::NotInstalled)?;
        if reason.as_deref() == Some(crate::mods::store::REASON_BLACKLIST_REMOVED) {
            return Err(Refusal::RemovedPending.into());
        }
        let (true, Some(sha256)) = (signed, sha256) else {
            // An unsigned file has no release to come back from (spec §1.3).
            return Err(Refusal::UnreadableRecord.into());
        };
        let url = updater::repair_url(id, &version).map_err(|_| Refusal::UnreadableRecord)?;
        Ok(RepairTarget {
            url,
            version,
            sha256,
        })
    }

    /// Put back the pinned version and file downloaded for `target`: a repair, which the ledger
    /// decides itself (same version, same sha256) and which re-checks the blacklist. `index` is
    /// whatever safety list is cached; a repair needs none, so an offline one still works.
    pub fn install_repair(
        &self,
        target: &RepairTarget,
        bytes: &[u8],
        keys: &TrustedKeys,
        index: Option<&Index>,
        now: DateTime<Utc>,
    ) -> Result<Installed, LedgerError> {
        let check = SafetyCheck::catalogue(
            index,
            now,
            Mode::Repair {
                version: target.version.clone(),
                sha256: target.sha256.clone(),
            },
        );
        let opts = InstallOptions {
            source: Source::Catalogue,
            consented: false,
            approval: Approval::Approved,
            catalogue: &check,
        };
        self.install_package(bytes, keys, &opts, now)
    }

    /// Every record this updater keeps: `(waiting updates, resumed mods)`. The Mods tab reads them
    /// with its rows in one step (`tab::listing::read_all`); this is the tests' read of the two.
    #[cfg(test)]
    pub fn notes(
        &self,
    ) -> Result<
        (
            std::collections::BTreeMap<String, Waiting>,
            Vec<blacklist::Resumed>,
        ),
        LedgerError,
    > {
        self.with_conn(|c| Ok((blacklist::read_waiting(c)?, blacklist::read_resumed(c)?)))
    }
}
