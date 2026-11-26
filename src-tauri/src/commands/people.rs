// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! People (Contacts, PR A): thin commands over `crate::people`, the one owner of the tables.
//!
//! Every command checks its window first (a read may come from the main window or the pop-out
//! Calendar, whose meeting card looks people up; everything else is the main window's), holds the
//! DB guard only for the work, drops it, and then announces the change once through
//! [`people::announce_changed`], so every open view re-reads. Nothing here calls a model, takes
//! `state.conn()` twice, or names a contact value: errors come from `people` as fixed words. No
//! command starts with `set_` (the Settings "Saved" tick). Not model-path code (E13): no model
//! path may name this module.
//!
//! Later slices of the same PR add the duplicate finder and merge, the vCard commands and the
//! timeline commands beside these; each adds its row to the window-tier table in the tests.

use std::time::Instant;

use tauri::{AppHandle, State};

use crate::error::Result;
use crate::people::dupes::DupesReport;
use crate::people::merge::{MergeChoices, MergeDone, MergePreview};
use crate::people::{
    self, Label, ListFilter, ListSort, Person, PersonDraft, PersonRow, PersonSaved,
};
use crate::AppState;

use super::shared::{require_people_main_window, require_people_window};

// --- reading (main window or the pop-out Calendar) --------------------------------------------------------

/// The People list: one filter, an optional search, one sort. People in Trash show only under
/// `Trash`.
#[tauri::command]
pub fn list_people(
    window: tauri::Window,
    state: State<'_, AppState>,
    filter: ListFilter,
    query: Option<String>,
    sort: ListSort,
) -> Result<Vec<PersonRow>> {
    require_people_window(&window)?;
    let conn = state.conn()?;
    people::list_people(&conn, filter, query.as_deref(), sort)
}

/// One person with everything (also one in Trash, so the Trash list can open them).
#[tauri::command]
pub fn get_person(window: tauri::Window, state: State<'_, AppState>, id: i64) -> Result<Person> {
    require_people_window(&window)?;
    let conn = state.conn()?;
    people::get_person(&conn, id)
}

/// Find people for Ctrl K and the pickers. Never returns anyone in Trash.
#[tauri::command]
pub fn search_people(
    window: tauri::Window,
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> Result<Vec<PersonRow>> {
    require_people_window(&window)?;
    let conn = state.conn()?;
    people::search_people(&conn, &query, limit)
}

// --- people (main window) ---------------------------------------------------------------------------------------

/// Add a person. Returns their id.
#[tauri::command]
pub fn create_person(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    draft: PersonDraft,
) -> Result<i64> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let id = people::create_person(&conn, &draft)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(id)
}

/// Save the whole record. Refuses with "changed elsewhere" when `base_updated_at` is no longer the
/// record's `updated_at` (the editor was opened on an older copy).
#[tauri::command]
pub fn update_person(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    draft: PersonDraft,
    base_updated_at: String,
) -> Result<PersonSaved> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let saved = people::update_person(&conn, id, &draft, &base_updated_at)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(saved)
}

/// Move people to Trash. `google_delete` names the Google sources the user also asked to delete
/// at Google; it is always empty until the Google PR. Returns how many moved.
#[tauri::command]
pub fn trash_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    google_delete: Vec<i64>,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let moved = people::trash_people(&conn, &ids, &google_delete)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(moved)
}

/// Bring people back from Trash. Returns how many came back.
#[tauri::command]
pub fn restore_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let restored = people::restore_people(&conn, &ids)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(restored)
}

/// Delete people for good. Only people already in Trash are deleted. Returns how many were.
#[tauri::command]
pub fn delete_people_now(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let deleted = people::delete_people_now(&conn, &ids)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(deleted)
}

/// Star or unstar people. Returns how many changed.
#[tauri::command]
pub fn star_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    starred: bool,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let changed = people::star_people(&conn, &ids, starred)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(changed)
}

// --- labels (main window) ----------------------------------------------------------------------------------------

/// Every label and how many people (outside Trash) carry it.
#[tauri::command]
pub fn list_labels(window: tauri::Window, state: State<'_, AppState>) -> Result<Vec<Label>> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::list_labels(&conn)
}

/// Add a label. Returns its id.
#[tauri::command]
pub fn create_label(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<i64> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let id = people::create_label(&conn, &name)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(id)
}

/// Rename a label.
#[tauri::command]
pub fn rename_label(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    label_id: i64,
    name: String,
) -> Result<()> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::rename_label(&conn, label_id, &name)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(())
}

/// Delete a label. Nobody is deleted with it. Returns how many people had it.
#[tauri::command]
pub fn delete_label(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    label_id: i64,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let had = people::delete_label(&conn, label_id)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(had)
}

/// Put a label on people. Returns how many gained it.
#[tauri::command]
pub fn label_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    label_id: i64,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let added = people::label_people(&conn, &ids, label_id)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(added)
}

/// Take a label off people. Returns how many carried it.
#[tauri::command]
pub fn unlabel_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    label_id: i64,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let removed = people::unlabel_people(&conn, &ids, label_id)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(removed)
}

// --- project links (main window) ---------------------------------------------------------------------------------

/// Link a person to a project by the name the user typed. Resolves to the project's own spelling.
#[tauri::command]
pub fn link_person_project(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    project: String,
) -> Result<String> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let name = people::link_person_project(&conn, id, &project)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(name)
}

/// Unlink a person from a project.
#[tauri::command]
pub fn unlink_person_project(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    project: String,
) -> Result<()> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::unlink_person_project(&conn, id, &project)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(())
}

// --- duplicates and merging (main window) -----------------------------------------------------------------------

/// The finder's snapshot: possible duplicates, grouped by what differs. The rows are read under the
/// DB guard, which is dropped before anything is compared (a big address book never holds the one
/// connection while it thinks).
#[tauri::command]
pub fn people_duplicates_snapshot(
    window: tauri::Window,
    state: State<'_, AppState>,
) -> Result<DupesReport> {
    require_people_main_window(&window)?;
    let input = {
        let conn = state.conn()?;
        people::dupes::load(&conn)?
    };
    Ok(people::dupes::scan(&input))
}

/// What merging `from` into `into` would move, and which single-value fields differ.
#[tauri::command]
pub fn merge_people_preview(
    window: tauri::Window,
    state: State<'_, AppState>,
    into: i64,
    from: i64,
) -> Result<MergePreview> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::merge::preview(&conn, into, from)
}

/// Merge `from` into `into`. `choices` answers each single-value field that differs; the value not
/// chosen is dropped. The answer carries the Undo token for 30 seconds.
#[tauri::command]
pub fn merge_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    into: i64,
    from: i64,
    choices: MergeChoices,
) -> Result<MergeDone> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let (mut done, step) = people::merge::merge_people(&conn, into, from, &choices)?;
    done.undo = people::shelve(&state.people_undo, state.store_epoch(), vec![step]);
    drop(conn);
    people::announce_changed(&app);
    Ok(done)
}

/// Merge every listed pair that loses nothing (each is judged again here). One Undo for all.
#[tauri::command]
pub fn merge_identical_pairs(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    pairs: Vec<(i64, i64)>,
) -> Result<MergeDone> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let (mut done, steps) = people::merge::merge_identical_pairs(&conn, &pairs)?;
    done.undo = people::shelve(&state.people_undo, state.store_epoch(), steps);
    drop(conn);
    people::announce_changed(&app);
    Ok(done)
}

/// Take a merge back, if its Undo is still good. Returns how many people came back.
#[tauri::command]
pub fn undo_merge_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let steps = people::unshelve(
        &state.people_undo,
        &token,
        state.store_epoch(),
        Instant::now(),
    )?;
    // A refusal (`Error::Other`) is final: the merge can't be undone any more. Anything else (a busy
    // or failing database) wrote nothing, so the offer goes back on the shelf for a retry.
    let back = match people::merge::undo_steps(&conn, &steps) {
        Ok(n) => n,
        Err(e) => {
            if !matches!(e, crate::error::Error::Other(_)) {
                people::reshelve(&state.people_undo, token, state.store_epoch(), steps);
            }
            return Err(e);
        }
    };
    drop(conn);
    people::announce_changed(&app);
    Ok(back)
}

/// Say two people are not the same: the finder never offers the pair again.
#[tauri::command]
pub fn mark_people_distinct(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    a: i64,
    b: i64,
) -> Result<()> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::dupes::mark_distinct(&conn, a, b)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(())
}

/// Offer the hidden pairs again. Returns how many were hidden.
#[tauri::command]
pub fn restore_people_distinct(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<usize> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let shown = people::dupes::restore_distinct(&conn)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which window a command may be called from.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Tier {
        /// The main window or the pop-out Calendar: `require_people_window`.
        Read,
        /// The main window only: `require_people_main_window`.
        Main,
    }

    /// Every people command and its tier (spec 2.3). Later slices add their commands here; the
    /// test below fails on a command in the file that is missing from this table, and on a table
    /// row whose command is not in the file.
    const TIERS: &[(&str, Tier)] = &[
        ("list_people", Tier::Read),
        ("get_person", Tier::Read),
        ("search_people", Tier::Read),
        ("create_person", Tier::Main),
        ("update_person", Tier::Main),
        ("trash_people", Tier::Main),
        ("restore_people", Tier::Main),
        ("delete_people_now", Tier::Main),
        ("star_people", Tier::Main),
        ("list_labels", Tier::Main),
        ("create_label", Tier::Main),
        ("rename_label", Tier::Main),
        ("delete_label", Tier::Main),
        ("label_people", Tier::Main),
        ("unlabel_people", Tier::Main),
        ("link_person_project", Tier::Main),
        ("unlink_person_project", Tier::Main),
        ("people_duplicates_snapshot", Tier::Main),
        ("merge_people_preview", Tier::Main),
        ("merge_people", Tier::Main),
        ("merge_identical_pairs", Tier::Main),
        ("undo_merge_people", Tier::Main),
        ("mark_people_distinct", Tier::Main),
        ("restore_people_distinct", Tier::Main),
    ];

    /// Main-window commands that only read (they announce nothing).
    const MAIN_READS: &[&str] = &[
        "list_labels",
        "people_duplicates_snapshot",
        "merge_people_preview",
    ];

    /// The shipped part of this file, and each command's body in it (name, body).
    fn commands() -> Vec<(String, String)> {
        let source = include_str!("people.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        shipped
            .split("#[tauri::command]\n")
            .skip(1)
            .map(|chunk| {
                let name = chunk
                    .strip_prefix("pub fn ")
                    .and_then(|rest| rest.split('(').next())
                    .expect("a command is `pub fn name(`")
                    .to_owned();
                // Comment lines dropped, so a guard named only in a comment proves nothing.
                let body = chunk
                    .split("\n}\n")
                    .next()
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                (name, body)
            })
            .collect()
    }

    /// Each command opens with its tier's guard, and the table and the file name the same
    /// commands. A command with no guard, or the wrong one, is how the briefing window (or the
    /// pop-out Calendar) ends up changing the address book.
    #[test]
    fn every_command_checks_the_window_its_tier_names() {
        let found = commands();
        let mut names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        let mut table: Vec<&str> = TIERS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        table.sort_unstable();
        assert_eq!(names, table, "the tier table and the commands disagree");
        for (name, body) in &found {
            let tier = TIERS.iter().find(|(n, _)| n == name).expect("listed").1;
            let (want, other) = match tier {
                Tier::Read => (
                    "require_people_window(&window)?;",
                    "require_people_main_window",
                ),
                Tier::Main => (
                    "require_people_main_window(&window)?;",
                    "require_people_window",
                ),
            };
            let at = body.find(want).unwrap_or(usize::MAX);
            let conn_at = body.find("state.conn()?").unwrap_or(usize::MAX);
            assert!(
                at < conn_at,
                "{name}: its guard comes before it reads the store"
            );
            assert!(
                !body.contains(other),
                "{name}: it names the other tier's guard"
            );
            assert!(
                !name.starts_with("set_"),
                "{name}: no command starts with `set_`"
            );
        }
    }

    /// A command that changes the address book drops the DB guard, then announces once; a read
    /// announces nothing. (`announce_changed` is the one emit, and the gate in
    /// `scripts/check-people-emit.mjs` keeps it so.)
    #[test]
    fn writes_announce_after_the_guard_drops_and_reads_do_not() {
        for (name, body) in commands() {
            let tier = TIERS.iter().find(|(n, _)| *n == name).expect("listed").1;
            let reads_only = tier == Tier::Read || MAIN_READS.contains(&name.as_str());
            if reads_only {
                assert!(
                    !body.contains("announce_changed"),
                    "{name}: a read announces"
                );
                continue;
            }
            assert_eq!(
                body.matches("people::announce_changed(&app);").count(),
                1,
                "{name}"
            );
            assert_eq!(
                body.matches("state.conn()?").count(),
                1,
                "{name}: one guard"
            );
            let dropped = body.find("drop(conn);").expect("it drops the guard");
            let announced = body.find("people::announce_changed(&app);").unwrap();
            assert!(
                dropped < announced,
                "{name}: the guard drops before the emit"
            );
            assert!(
                body.find("people::").unwrap() < dropped,
                "{name}: the write is under the guard"
            );
        }
    }

    /// The argument shapes the webview sends: bare words for the unit variants, `{"Label": 3}`
    /// for the one that carries an id; a draft's birthday travels as `birthday`; a draft with
    /// fields left out is still a draft.
    #[test]
    fn the_wire_shapes_the_frontend_sends_parse() {
        for (json, want) in [
            ("\"All\"", ListFilter::All),
            ("\"Starred\"", ListFilter::Starred),
            ("\"Trash\"", ListFilter::Trash),
            ("\"Unlabelled\"", ListFilter::Unlabelled),
            ("{\"Label\":3}", ListFilter::Label(3)),
        ] {
            assert_eq!(
                serde_json::from_str::<ListFilter>(json).unwrap(),
                want,
                "{json}"
            );
        }
        for (json, want) in [
            ("\"First\"", ListSort::First),
            ("\"Last\"", ListSort::Last),
            ("\"RecentlyMet\"", ListSort::RecentlyMet),
        ] {
            assert_eq!(
                serde_json::from_str::<ListSort>(json).unwrap(),
                want,
                "{json}"
            );
        }
        assert!(serde_json::from_str::<ListFilter>("\"Everyone\"").is_err());

        let draft: PersonDraft = serde_json::from_str(
            "{\"display_name\":\"Sam Example\",\"birthday\":\"--03-04\",\
              \"details\":[{\"kind\":\"email\",\"value\":\"sam@example.com\"}]}",
        )
        .unwrap();
        assert_eq!(draft.display_name, "Sam Example");
        assert_eq!(draft.born_on.as_deref(), Some("--03-04"));
        assert_eq!(draft.details.len(), 1);
    }
}
