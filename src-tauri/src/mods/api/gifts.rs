// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a mod may give (PR C, K1): the gift quota. A gift is a task or a milestone a mod adds to the
//! user's lists; each one is attributed by a `mod_ledger` row written in the same transaction as the
//! gift (K0's `*_in(&Connection)` seams). A mod may have made at most
//! [`limits::MAX_GIFTS_PER_MOD`] of them, ever: the count is of its ledger rows, which an uninstall
//! keeps (`mod_ledger` has no foreign key to `mods`, by design), so a reinstall does not reset it.
//!
//! The count is taken INSIDE the add's transaction, through the same connection, so two adds racing
//! cannot both take the last place, and a gift whose transaction rolls back counts for nothing. It
//! reads `idx_mod_ledger_mod`. Past the quota the call is answered `MOD-3003`.

use rusqlite::{params, Connection, OptionalExtension};

use super::{limits, Code, ModError};
use crate::mods::ids::ModId;
use crate::mods::policy::{self, Maker};

/// Whether `mod_id` may make `adding` more gifts: it has made, plus `adding`, no more than
/// [`limits::MAX_GIFTS_PER_MOD`]. A batch (a checklist import writes one ledger row per task) passes
/// its whole size, so it cannot step over the quota by being checked as one. Call it through the
/// connection (or transaction) the gifts are written with, before writing.
pub(super) fn ensure_room(conn: &Connection, mod_id: &ModId, adding: u64) -> Result<(), ModError> {
    ensure_room_under(conn, mod_id, adding, limits::MAX_GIFTS_PER_MOD)
}

fn ensure_room_under(
    conn: &Connection,
    mod_id: &ModId,
    adding: u64,
    max: u64,
) -> Result<(), ModError> {
    let made: i64 = conn
        .query_row(
            "SELECT count(*) FROM mod_ledger WHERE mod_id = ?1",
            params![mod_id.as_str()],
            |r| r.get(0),
        )
        .map_err(ModError::internal)?;
    if u64::try_from(made)
        .unwrap_or(u64::MAX)
        .saturating_add(adding)
        > max
    {
        return Err(ModError::new(
            Code::QuotaExceeded,
            "This mod has made as many tasks and milestones as PM allows one mod.",
        ));
    }
    Ok(())
}

/// What a mod made: one task or one milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Gift {
    Task(i64),
    Milestone(i64),
}

/// Attribute `gift` to `mod_id`: its `mod_ledger` row, written through the same connection (the
/// transaction) as the gift, from the mod's own row as it is now (its name, tier, publisher, whether
/// it was signed, and its install id, which is how an unsigned maker is told from a later install of
/// the same id). A mod with no row (uninstalled while the call ran) made nothing: the call ended.
pub(super) fn record(conn: &Connection, mod_id: &ModId, gift: Gift) -> Result<(), ModError> {
    let (task, milestone) = match gift {
        Gift::Task(id) => (Some(id), None),
        Gift::Milestone(id) => (None, Some(id)),
    };
    let wrote = conn
        .execute(
            "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, \
                                    install_id, task_id, milestone_id) \
             SELECT id, name, tier, author, source_repo, signed, install_id, ?2, ?3 \
             FROM mods WHERE id = ?1",
            params![mod_id.as_str(), task, milestone],
        )
        .map_err(ModError::internal)?;
    if wrote == 1 {
        Ok(())
    } else {
        Err(ModError::session_ended())
    }
}

/// Whether the installed mod `mod_id` made milestone `milestone` itself (`policy::made_it`: a
/// reinstall of a signed mod may edit its own gifts again; an unsigned one only the same install).
/// A milestone with no ledger row (the user's own, or one PM made) is nobody's gift.
pub(super) fn made_milestone(
    conn: &Connection,
    mod_id: &ModId,
    milestone: i64,
) -> Result<bool, ModError> {
    let maker = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Maker> {
        Ok(Maker {
            mod_id: mod_id.as_str().to_owned(),
            signed: r.get(0)?,
            tier: r.get(1)?,
            author: r.get(2)?,
            source_repo: r.get(3)?,
            install_id: r.get(4)?,
        })
    };
    let made: Option<Maker> = conn
        .query_row(
            "SELECT signed, tier, author, source_repo, install_id FROM mod_ledger \
             WHERE milestone_id = ?1 AND mod_id = ?2",
            params![milestone, mod_id.as_str()],
            maker,
        )
        .optional()
        .map_err(ModError::internal)?;
    let Some(made) = made else {
        return Ok(false);
    };
    let current: Option<Maker> = conn
        .query_row(
            "SELECT signed, tier, author, source_repo, install_id FROM mods WHERE id = ?1",
            params![mod_id.as_str()],
            maker,
        )
        .optional()
        .map_err(ModError::internal)?;
    Ok(current.is_some_and(|current| policy::made_it(&made, &current)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::test_support::{insert_mod, ModRow, TestHost};

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    /// `n` tasks, each made by `mod_id` (as its `mods` row says it is), through the ledger's own
    /// columns.
    fn gifts(host: &TestHost, mod_id: &str, n: i64) {
        host.sql(|c| {
            c.execute(
                "INSERT INTO tasks(title) \
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1) \
                 SELECT 'gift ' || i FROM n",
                [n],
            )
            .unwrap();
            // The newest `n` tasks are this mod's.
            c.execute(
                "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, \
                                        install_id, task_id) \
                 SELECT m.id, 'Test', m.tier, m.author, m.source_repo, m.signed, m.install_id, t.id \
                 FROM mods m, (SELECT id FROM tasks ORDER BY id DESC LIMIT ?2) t WHERE m.id = ?1",
                params![mod_id, n],
            )
            .unwrap();
        });
    }

    #[test]
    fn a_mod_may_make_ten_thousand_and_not_one_more() {
        let host = TestHost::new();
        host.sql(|c| {
            insert_mod(c, &ModRow::signed("acme.giver", "second", &["tasks.write"]));
            insert_mod(c, &ModRow::signed("acme.other", "second", &["tasks.write"]));
        });
        let max = limits::MAX_GIFTS_PER_MOD as i64;
        gifts(&host, "acme.giver", max - 1);
        host.sql(|c| ensure_room(c, &id("acme.giver"), 1)).unwrap();
        // The tenth thousandth is the last: with it made, the next is refused.
        gifts(&host, "acme.giver", 1);
        let e = host
            .sql(|c| ensure_room(c, &id("acme.giver"), 1))
            .unwrap_err();
        assert_eq!(e.code, Code::QuotaExceeded);
        // Another mod's gifts are its own count, and a mod with none has room.
        host.sql(|c| ensure_room(c, &id("acme.other"), 1)).unwrap();
        gifts(&host, "acme.other", 5);
        host.sql(|c| ensure_room(c, &id("acme.other"), 1)).unwrap();
    }

    /// A batch is checked for its whole size: at the boundary, 200 more do not fit when only one does.
    #[test]
    fn a_batch_is_refused_when_it_would_step_over_the_quota() {
        let host = TestHost::new();
        host.sql(|c| insert_mod(c, &ModRow::signed("acme.giver", "second", &["tasks.write"])));
        let max = limits::MAX_GIFTS_PER_MOD;
        gifts(&host, "acme.giver", max as i64 - 1);
        host.sql(|c| ensure_room(c, &id("acme.giver"), 1)).unwrap();
        let e = host
            .sql(|c| ensure_room(c, &id("acme.giver"), 200))
            .unwrap_err();
        assert_eq!(e.code, Code::QuotaExceeded);
        // A batch that lands exactly on the quota is fine; one more is not.
        host.sql(|c| ensure_room_under(c, &id("acme.giver"), 3, max + 2))
            .unwrap();
        let e = host
            .sql(|c| ensure_room_under(c, &id("acme.giver"), 4, max + 2))
            .unwrap_err();
        assert_eq!(e.code, Code::QuotaExceeded);
    }

    /// A reinstall doesn't reset it: the ledger outlives the mod's row.
    #[test]
    fn an_uninstall_keeps_the_count_so_a_reinstall_starts_where_it_left_off() {
        let host = TestHost::new();
        host.sql(|c| insert_mod(c, &ModRow::signed("acme.giver", "second", &[])));
        gifts(&host, "acme.giver", 3);
        host.sql(|c| {
            c.execute("DELETE FROM mods WHERE id = 'acme.giver'", [])
                .unwrap()
        });
        let e = host
            .sql(|c| ensure_room_under(c, &id("acme.giver"), 1, 3))
            .unwrap_err();
        assert_eq!(e.code, Code::QuotaExceeded);
        host.sql(|c| ensure_room_under(c, &id("acme.giver"), 1, 4))
            .unwrap();
    }

    /// The count sees what its own transaction has written and not what a rollback undid.
    #[test]
    fn the_count_is_taken_inside_the_transaction_and_a_rollback_counts_for_nothing() {
        let host = TestHost::new();
        host.sql(|c| insert_mod(c, &ModRow::signed("acme.giver", "second", &[])));
        host.sql(|c| {
            let tx = c.unchecked_transaction().unwrap();
            tx.execute("INSERT INTO tasks(title) VALUES ('a')", [])
                .unwrap();
            let task = tx.last_insert_rowid();
            let ledger = |tx: &Connection| {
                tx.execute(
                    "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, \
                                            install_id, task_id) \
                     SELECT id, 'Test', tier, author, source_repo, signed, install_id, ?1 \
                     FROM mods WHERE id = 'acme.giver'",
                    [task],
                )
                .unwrap();
            };
            ledger(&tx);
            // Inside the transaction its own gift is counted.
            assert_eq!(
                ensure_room_under(&tx, &id("acme.giver"), 1, 1)
                    .unwrap_err()
                    .code,
                Code::QuotaExceeded
            );
            tx.rollback().unwrap();
            // After the rollback, nothing was made.
            ensure_room_under(c, &id("acme.giver"), 1, 1).unwrap();
        });
    }
}
