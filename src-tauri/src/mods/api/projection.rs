// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a DTO may carry for this caller (PR C, K1). A task or a milestone row touches the user's
//! projects and calendar (a project's name, the event a deadline follows), and a mod holding
//! `tasks.read` alone must not learn those. A [`Projection`] says, from the caller's grants, which of
//! the two the caller may see, and a [`Dto`] is built field by field from named columns, each field
//! saying which projection it needs: the pattern of `documents.rs` (no whole-struct serialisation,
//! so a column added to a table can never leak by being added), shared by every PR C call.
//!
//! The projection is a plain value, not a grant lookup, on purpose: the read-only MCP server (C3)
//! builds the same DTOs from the scopes its client was given ([`Projection::scoped`]), with no mod
//! and no `mods` row in sight.

// Nothing calls the projection until K2's handlers (and C3's MCP) do.
#![cfg_attr(not(test), allow(dead_code))]

use rusqlite::Connection;
use serde_json::{Map, Value};

use super::grants::{self, ModFacts};
use super::{Code, ModError};

/// Which of the user's other data a caller may see alongside what it asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Projection {
    /// Project names and ids (`projects.read`).
    pub projects: bool,
    /// Calendar events a deadline or a task follows (`calendar.read`).
    pub calendar: bool,
}

impl Projection {
    /// A caller that sees neither.
    pub const NONE: Self = Self {
        projects: false,
        calendar: false,
    };

    /// A caller given exactly these scopes (the MCP server's clients).
    pub const fn scoped(projects: bool, calendar: bool) -> Self {
        Self { projects, calendar }
    }

    /// What a mod's grants allow now: `projects.read` and `calendar.read` each checked as the
    /// dispatcher checks any permission ([`grants::check`], D3 and the sealed engine included), under
    /// the guard `conn` is. Not allowed for any reason (undeclared, denied, not asked yet) is simply
    /// not in the projection; only PM's own failure is an error.
    pub fn of_grants(
        conn: &Connection,
        facts: &ModFacts,
        engine_sealed: bool,
    ) -> Result<Self, ModError> {
        let held = |permission: &'static str| match grants::check(
            conn,
            facts,
            permission,
            engine_sealed,
        ) {
            Ok(()) => Ok(true),
            Err(e) if matches!(e.code, Code::NotGranted | Code::NeedsPrompt) => Ok(false),
            Err(e) => Err(e),
        };
        Ok(Self {
            projects: held("projects.read")?,
            calendar: held("calendar.read")?,
        })
    }
}

/// What a field needs of the projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Always: the call's own data.
    Nothing,
    /// Only with [`Projection::projects`].
    Projects,
    /// Only with [`Projection::calendar`].
    Calendar,
}

/// One DTO under construction.
#[derive(Debug, Clone)]
pub struct Dto {
    projection: Projection,
    fields: Map<String, Value>,
}

impl Dto {
    pub fn new(projection: Projection) -> Self {
        Self {
            projection,
            fields: Map::new(),
        }
    }

    /// Put `value` under `name`, if the caller's projection allows a field that `needs` it. A field
    /// that isn't allowed is not there at all: not `null`, which a reader could take for "no value".
    pub fn field(mut self, needs: Needs, name: &'static str, value: impl Into<Value>) -> Self {
        let allowed = match needs {
            Needs::Nothing => true,
            Needs::Projects => self.projection.projects,
            Needs::Calendar => self.projection.calendar,
        };
        if allowed {
            self.fields.insert(name.to_owned(), value.into());
        }
        self
    }

    pub fn build(self) -> Value {
        Value::Object(self.fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::test_support::{insert_mod, ModRow, TestHost};
    use crate::mods::ids::ModId;
    use serde_json::json;

    fn dto(p: Projection) -> Value {
        Dto::new(p)
            .field(Needs::Nothing, "id", 7)
            .field(Needs::Nothing, "title", "Call Sam")
            .field(Needs::Projects, "projectName", "Alpha")
            .field(Needs::Calendar, "eventTitle", "Sprint review")
            // Present-or-absent, never null for "not allowed".
            .field(Needs::Projects, "milestoneLabel", Value::Null)
            .build()
    }

    /// The grid: each field is there exactly when the projection allows it, and the keys are the
    /// ones named (nothing else can appear).
    #[test]
    fn a_dto_has_exactly_the_fields_its_projection_allows() {
        assert_eq!(dto(Projection::NONE), json!({"id": 7, "title": "Call Sam"}));
        assert_eq!(
            dto(Projection::scoped(true, false)),
            json!({"id": 7, "title": "Call Sam", "projectName": "Alpha", "milestoneLabel": null})
        );
        assert_eq!(
            dto(Projection::scoped(false, true)),
            json!({"id": 7, "title": "Call Sam", "eventTitle": "Sprint review"})
        );
        assert_eq!(
            dto(Projection::scoped(true, true)),
            json!({
                "id": 7, "title": "Call Sam", "projectName": "Alpha",
                "eventTitle": "Sprint review", "milestoneLabel": null
            })
        );
        assert_eq!(Projection::default(), Projection::NONE);
    }

    fn facts(host: &TestHost, declared: &[&'static str], granted: &[&str]) -> ModFacts {
        host.sql(|c| {
            insert_mod(c, &ModRow::signed("atlas.reader", "first", declared));
            for p in granted {
                c.execute(
                    "INSERT INTO mod_grants(mod_id, permission, decision) \
                     VALUES ('atlas.reader', ?1, 'granted')",
                    [p],
                )
                .unwrap();
            }
            grants::read_mod(c, &ModId::parse("atlas.reader").unwrap())
                .unwrap()
                .unwrap()
        })
    }

    /// A caller's projection comes from its grants: a held, declared read is in; undeclared,
    /// declared but never consented to, or denied is out.
    #[test]
    fn a_projection_follows_the_callers_grants() {
        let both = ["projects.read", "calendar.read"];
        let cases: [(&[&'static str], &[&str], Projection); 4] = [
            (&both, &both, Projection::scoped(true, true)),
            (&both, &["projects.read"], Projection::scoped(true, false)),
            (
                &["projects.read"],
                &["projects.read"],
                Projection::scoped(true, false),
            ),
            (&["tasks.read"], &["tasks.read"], Projection::NONE),
        ];
        for (declared, granted, want) in cases {
            let host = TestHost::new();
            let facts = facts(&host, declared, granted);
            let got = host
                .sql(|c| Projection::of_grants(c, &facts, true))
                .unwrap();
            assert_eq!(got, want, "{declared:?} {granted:?}");
        }
        // A permission the user turned off after granting it is out.
        let host = TestHost::new();
        let facts = facts(&host, &both, &both);
        host.sql(|c| {
            c.execute(
                "UPDATE mod_grants SET decision = 'denied' WHERE permission = 'calendar.read'",
                [],
            )
            .unwrap()
        });
        assert_eq!(
            host.sql(|c| Projection::of_grants(c, &facts, true))
                .unwrap(),
            Projection::scoped(true, false)
        );
    }
}
