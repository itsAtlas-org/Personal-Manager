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
//! The tiers: a read, and the meeting card's three commands (`owed_meetings`, `event_guests`,
//! `answer_meeting`),
//! may come from the main window or the pop-out Calendar, where an event's "Log who you met" opens
//! the same card; everything else is the main window's. A table in the tests pins every command's
//! tier.

use std::time::Instant;

use tauri::{AppHandle, Manager, State};

use crate::blocking::spawn_blocking_result;
use crate::calendar::Attendee;
use crate::error::{Error, Result};
use crate::people::dupes::DupesReport;
use crate::people::export::{ExportDone, ExportScope};
use crate::people::import::{ImportOptions, ImportPreview, ImportResult};
use crate::people::meet::{EventGuests, MeetingAnswer, MeetingAnswered, OwedMeeting, PersonRef};
use crate::people::merge::{MergeChoices, MergeDone, MergePreview};
use crate::people::timeline::{InteractionDraft, TimelineCursor, TimelinePage};
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

// --- vCard import and export (main window) ---------------------------------------------------------------------

/// The longest a file's name is shown (it is the user's own text, but a hostile name is still text).
const FILE_NAME_CHARS: usize = 120;

/// Pick a `.vcf` file and read what is in it, writing nothing (spec 5.2). The backend opens the
/// picker, so the path never round-trips through the webview. The file is read with a cap and
/// parsed off the DB guard; the parse is then held in memory for ten minutes under the returned
/// `preview_id`, and a new preview replaces it. A cancelled picker answers `None`.
#[tauri::command]
pub async fn pick_vcard_and_preview(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ImportPreview>> {
    use tauri_plugin_dialog::DialogExt;
    require_people_main_window(&window)?;
    let app2 = app.clone();
    // Deliberately NOT `blocking::spawn_blocking_result`: the picker returns an `Option`, not a
    // `Result` (the `set_proton_cli_path` shape).
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .file()
            .set_title("Choose a contacts file")
            .add_filter("vCard", &["vcf", "vcard"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| Error::Other("The file dialog didn't open.".into()))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|_| Error::Other("PM couldn't read the chosen file's place.".into()))?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .map(|n| people::card::truncate_chars(&people::fold::clean_text(&n), FILE_NAME_CHARS).0)
        .unwrap_or_default();
    // Parse, then count against the store, both off the async worker: counting is a few lookups
    // per card. The epoch is read under the guard, so a store that changes afterwards makes the
    // hold stale (a refusal), never wrong.
    let app2 = app.clone();
    let (parsed, counts, epoch) = spawn_blocking_result("contacts file", move || {
        let bytes = people::import::read_capped(&path)?;
        let parsed = people::import::prepare(people::vcard::parse_vcf(&bytes));
        let state = app2.state::<AppState>();
        let conn = state.conn()?;
        let epoch = state.store_epoch();
        let counts = people::import::preview_counts(&conn, &parsed.cards)?;
        Ok((parsed, counts, epoch))
    })
    .await?;
    let mut preview = ImportPreview::of(String::new(), file_name, &parsed, counts);
    let held = state
        .people_import
        .lock()
        .map_err(|_| Error::Other("PM couldn't hold that file.".into()))?
        .put(epoch, parsed.cards, Instant::now())?;
    preview.preview_id = held;
    Ok(Some(preview))
}

/// Import the file read by the last preview, as one transaction, then tell every view once. A
/// preview that was replaced, is over ten minutes old or was read under another store refuses in
/// plain words and writes nothing.
#[tauri::command]
pub async fn commit_vcard_import(
    window: tauri::Window,
    app: AppHandle,
    preview_id: String,
    options: ImportOptions,
) -> Result<ImportResult> {
    require_people_main_window(&window)?;
    // A big file is thousands of writes and a duplicate scan: off the main thread, which would
    // otherwise freeze the window for it.
    let app2 = app.clone();
    let done = spawn_blocking_result("contacts import", move || {
        let state = app2.state::<AppState>();
        let conn = state.conn()?;
        let mut done = people::import::commit_held(
            &conn,
            &state.people_import,
            state.store_epoch(),
            &preview_id,
            &options,
            Instant::now(),
        )?;
        // The review count is a courtesy: the import is already written, so a failure to read for
        // it must not turn a finished import into an error (or skip telling the views).
        let dupes = people::dupes::load(&conn).ok();
        drop(conn);
        done.review = dupes.map_or(0, |d| people::import::review_count(&d, &done.touched));
        Ok(done)
    })
    .await?;
    people::announce_changed(&app);
    Ok(done)
}

/// Write people as a vCard 3.0 file, and with `timeline` their timeline as a spreadsheet beside it
/// (spec 5.3). The backend opens the save dialog (rule L-5: decrypted data is never written to a
/// path the webview names). People in Trash are never written. A cancelled dialog answers `None`.
#[tauri::command]
pub async fn export_people(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    scope: ExportScope,
    timeline: bool,
) -> Result<Option<ExportDone>> {
    use tauri_plugin_dialog::DialogExt;
    require_people_main_window(&window)?;
    let exported = {
        let conn = state.conn()?;
        people::export::build(&conn, &scope, timeline)?
    };
    let app2 = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .file()
            .set_title("Export people")
            .set_file_name("people.vcf")
            .add_filter("vCard", &["vcf"])
            .blocking_save_file()
    })
    .await
    .map_err(|_| Error::Other("The file dialog didn't open.".into()))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let dest = picked
        .into_path()
        .map_err(|_| Error::Other("PM couldn't read the chosen file's place.".into()))?;
    let done = spawn_blocking_result("contacts export", move || {
        people::export::write(&dest, &exported)
    })
    .await?;
    Ok(Some(done))
}

// --- the timeline (main window) ------------------------------------------------------------------------------

/// One page of a person's timeline, newest first. `before` is the previous page's `next`.
#[tauri::command]
pub fn list_timeline(
    window: tauri::Window,
    state: State<'_, AppState>,
    person_id: i64,
    before: Option<TimelineCursor>,
    limit: Option<usize>,
) -> Result<TimelinePage> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::timeline::list_timeline(
        &conn,
        person_id,
        before.as_ref(),
        limit.unwrap_or(people::timeline::PAGE),
    )
}

/// Log a meeting, call, message or note naming one or more people. Never changes the people's own
/// records, so an open editor still saves. Returns the entry's id.
#[tauri::command]
pub fn add_interaction(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    draft: InteractionDraft,
) -> Result<i64> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    let id = people::timeline::add_interaction(&conn, &draft)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(id)
}

/// Save an edit over an entry: its kind, date, time, note and people (its meeting link stays).
#[tauri::command]
pub fn update_interaction(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    draft: InteractionDraft,
) -> Result<()> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::timeline::update_interaction(&conn, id, &draft)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(())
}

/// Delete an entry, for everyone it names.
#[tauri::command]
pub fn delete_interaction(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<()> {
    require_people_main_window(&window)?;
    let conn = state.conn()?;
    people::timeline::delete_interaction(&conn, id)?;
    drop(conn);
    people::announce_changed(&app);
    Ok(())
}

// --- "who did you meet?" (main window or the pop-out Calendar) -----------------------------------------------

/// The meetings owed an answer now, newest first (spec 6.2): recent, with other human guests,
/// and not yet answered.
#[tauri::command]
pub fn owed_meetings(
    window: tauri::Window,
    state: State<'_, AppState>,
) -> Result<Vec<OwedMeeting>> {
    require_people_window(&window)?;
    let conn = state.conn()?;
    people::meet::owed_meetings(&conn)
}

/// An event's guests sorted for the meeting card (the people PM knows, and those it doesn't): what
/// "Log who you met" opens on a past event that is not on the owed list. The guests are the ones the
/// calendar already showed the caller; nothing is stored or announced.
#[tauri::command]
pub fn event_guests(
    window: tauri::Window,
    state: State<'_, AppState>,
    guests: Vec<Attendee>,
) -> Result<EventGuests> {
    require_people_window(&window)?;
    let conn = state.conn()?;
    people::meet::event_guests(&conn, &guests)
}

/// Answer one meeting in a single step: log who was there (guests PM doesn't know become people,
/// anyone ticked from Trash comes back), skip it, or skip its whole series. Answering again
/// replaces an earlier skip. `event_ical` is empty for an event with no iCal id, which can only be
/// logged. `title` defaults to the mirrored event's.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn answer_meeting(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    event_ical: String,
    start: String,
    people: Vec<PersonRef>,
    answer: MeetingAnswer,
    note: Option<String>,
    title: Option<String>,
) -> Result<MeetingAnswered> {
    require_people_window(&window)?;
    let conn = state.conn()?;
    let done = people::meet::answer_meeting(
        &conn,
        &event_ical,
        &start,
        &people,
        answer,
        note.as_deref(),
        title.as_deref(),
    )?;
    drop(conn);
    people::announce_changed(&app);
    Ok(done)
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
        ("pick_vcard_and_preview", Tier::Main),
        ("commit_vcard_import", Tier::Main),
        ("export_people", Tier::Main),
        ("list_timeline", Tier::Main),
        ("add_interaction", Tier::Main),
        ("update_interaction", Tier::Main),
        ("delete_interaction", Tier::Main),
        ("owed_meetings", Tier::Read),
        ("event_guests", Tier::Read),
        ("answer_meeting", Tier::Read),
    ];

    /// Commands the pop-out Calendar may call that also write: the meeting card answers from there.
    const READ_TIER_WRITES: &[&str] = &["answer_meeting"];

    /// Main-window commands that only read (they announce nothing).
    const MAIN_READS: &[&str] = &[
        "list_labels",
        "people_duplicates_snapshot",
        "merge_people_preview",
        // The import's preview and the export write nothing in the store: the file is the write.
        "pick_vcard_and_preview",
        "export_people",
        "list_timeline",
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
                    .strip_prefix("pub async fn ")
                    .or_else(|| chunk.strip_prefix("pub fn "))
                    .and_then(|rest| rest.split('(').next())
                    .expect("a command is `pub fn name(` or `pub async fn name(`")
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
            let reads_only = (tier == Tier::Read && !READ_TIER_WRITES.contains(&name.as_str()))
                || MAIN_READS.contains(&name.as_str());
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
