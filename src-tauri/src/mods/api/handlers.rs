// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The registered handlers: one [`Method`] per live call row in the table (a test holds the two
//! together), and the step that runs the one a call names. A handler gets a call that passed every
//! check, and the arguments already parsed as JSON; it validates their shape itself, and touches
//! data only through [`super::recheck`] under the guard it uses.

use std::future::Future;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{
    ai, doc_storage, documents, embeddings, limits, link_calls, milestone_calls, net_fetch,
    note_calls, overlay_calls, project_calls, storage, surface, task_calls, Authorised, Host,
    ModError, PageAction, Reply,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    ThemeGet,
    StorageGet,
    StorageSet,
    StorageDocGet,
    StorageDocSet,
    StorageDocList,
    StorageDocDelete,
    DocumentsList,
    EmbeddingsDocumentVectors,
    EmbeddingsSignature,
    UiOpenDocument,
    AiComplete,
    NetFetch,
    SecretsSet,
    Notify,
    TasksList,
    TasksAdd,
    TasksSetStatus,
    TasksReorder,
    TasksImportChecklist,
    MilestonesList,
    MilestonesAdd,
    MilestonesUpdate,
    MilestonesSetStatus,
    MilestonesDelete,
    ProjectsList,
    DocumentsSaveNote,
    DocumentsStatusOf,
    UiOpenTaskEditor,
    ShellOpenLink,
    CalendarOverlaySet,
    /// The second-READ table test's own write call (`second_read::probe`): in no `ALL`, no dispatch
    /// table of PM's, and no build but a test's.
    #[cfg(test)]
    SecondReadProbe,
}

impl Method {
    pub const ALL: [Method; 31] = [
        Self::ThemeGet,
        Self::StorageGet,
        Self::StorageSet,
        Self::StorageDocGet,
        Self::StorageDocSet,
        Self::StorageDocList,
        Self::StorageDocDelete,
        Self::DocumentsList,
        Self::EmbeddingsDocumentVectors,
        Self::EmbeddingsSignature,
        Self::UiOpenDocument,
        Self::AiComplete,
        Self::NetFetch,
        Self::SecretsSet,
        Self::Notify,
        Self::TasksList,
        Self::TasksAdd,
        Self::TasksSetStatus,
        Self::TasksReorder,
        Self::TasksImportChecklist,
        Self::MilestonesList,
        Self::MilestonesAdd,
        Self::MilestonesUpdate,
        Self::MilestonesSetStatus,
        Self::MilestonesDelete,
        Self::ProjectsList,
        Self::DocumentsSaveNote,
        Self::DocumentsStatusOf,
        Self::UiOpenTaskEditor,
        Self::ShellOpenLink,
        Self::CalendarOverlaySet,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::ThemeGet => "theme.get",
            Self::StorageGet => "storage.get",
            Self::StorageSet => "storage.set",
            Self::StorageDocGet => "storage.doc.get",
            Self::StorageDocSet => "storage.doc.set",
            Self::StorageDocList => "storage.doc.list",
            Self::StorageDocDelete => "storage.doc.delete",
            Self::DocumentsList => "documents.list",
            Self::EmbeddingsDocumentVectors => "embeddings.documentVectors",
            Self::EmbeddingsSignature => "embeddings.signature",
            Self::UiOpenDocument => "ui.openDocument",
            Self::AiComplete => "ai.complete",
            Self::NetFetch => "net.fetch",
            Self::SecretsSet => "secrets.set",
            Self::Notify => "notify",
            Self::TasksList => "tasks.list",
            Self::TasksAdd => "tasks.add",
            Self::TasksSetStatus => "tasks.setStatus",
            Self::TasksReorder => "tasks.reorder",
            Self::TasksImportChecklist => "tasks.importChecklist",
            Self::MilestonesList => "milestones.list",
            Self::MilestonesAdd => "milestones.add",
            Self::MilestonesUpdate => "milestones.update",
            Self::MilestonesSetStatus => "milestones.setStatus",
            Self::MilestonesDelete => "milestones.delete",
            Self::ProjectsList => "projects.list",
            Self::DocumentsSaveNote => "documents.saveNote",
            Self::DocumentsStatusOf => "documents.statusOf",
            Self::UiOpenTaskEditor => "ui.openTaskEditor",
            Self::ShellOpenLink => "shell.openLink",
            Self::CalendarOverlaySet => "calendar.overlay.set",
            #[cfg(test)]
            Self::SecondReadProbe => super::second_read::PROBE_CALL,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let live = Self::ALL.into_iter().find(|m| m.name() == name);
        #[cfg(test)]
        let live = live
            .or_else(|| (name == super::second_read::PROBE_CALL).then_some(Self::SecondReadProbe));
        live
    }
}

/// Run the handler of `auth`'s row.
pub(super) async fn run<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let Some(method) = Method::from_name(auth.row.name) else {
        // A live row without a handler: the coverage test forbids it.
        return Err(ModError::internal(format!(
            "no handler for {}",
            auth.row.name
        )));
    };
    match method {
        Method::ThemeGet => theme_get(args),
        Method::StorageGet => storage::get(host, auth, args).await,
        Method::StorageSet => storage::set(host, auth, args).await,
        Method::StorageDocGet => doc_storage::get(host, auth, args).await,
        Method::StorageDocSet => doc_storage::set(host, auth, args).await,
        Method::StorageDocList => doc_storage::list(host, auth, args).await,
        Method::StorageDocDelete => doc_storage::delete(host, auth, args).await,
        Method::DocumentsList => documents::list(host, auth, args).await,
        Method::EmbeddingsDocumentVectors => embeddings::document_vectors(host, auth, args).await,
        Method::EmbeddingsSignature => embeddings::signature(host, auth, args).await,
        Method::UiOpenDocument => documents::open_document(host, auth, args).await,
        Method::AiComplete => ai::complete(host, auth, args).await,
        Method::NetFetch => net_fetch::fetch(host, auth, args).await,
        Method::SecretsSet => surface::secrets_set(host, auth, args).await,
        Method::Notify => surface::notify(host, auth, args).await,
        Method::TasksList => task_calls::list(host, auth, args).await,
        Method::TasksAdd => task_calls::add(host, auth, args).await,
        Method::TasksSetStatus => task_calls::set_status(host, auth, args).await,
        Method::TasksReorder => task_calls::reorder(host, auth, args).await,
        Method::TasksImportChecklist => task_calls::import_checklist(host, auth, args).await,
        Method::MilestonesList => milestone_calls::list(host, auth, args).await,
        Method::MilestonesAdd => milestone_calls::add(host, auth, args).await,
        Method::MilestonesUpdate => milestone_calls::update(host, auth, args).await,
        Method::MilestonesSetStatus => milestone_calls::set_status(host, auth, args).await,
        Method::MilestonesDelete => milestone_calls::delete(host, auth, args).await,
        Method::ProjectsList => project_calls::list(host, auth, args).await,
        Method::DocumentsSaveNote => note_calls::save_note(host, auth, args).await,
        Method::DocumentsStatusOf => note_calls::status_of(host, auth, args).await,
        Method::UiOpenTaskEditor => task_calls::open_editor(host, auth, args).await,
        Method::ShellOpenLink => link_calls::open_link(host, auth, args).await,
        Method::CalendarOverlaySet => overlay_calls::set(host, auth, args).await,
        #[cfg(test)]
        Method::SecondReadProbe => super::second_read::probe(host, auth, args).await,
    }
}

/// `theme.get()`: PM's live theme. Rust has authorised it; the page answers from its theme
/// (`ModFrame.tsx`, `modTheme.ts`), which holds the CSS custom properties and the axes.
fn theme_get(args: &Value) -> Result<Reply, ModError> {
    no_args(args)?;
    Ok(Reply::Page {
        action: PageAction::ThemeGet,
        args: Value::Null,
    })
}

/// A call that takes nothing takes `null` (or nothing) or `{}`.
pub(super) fn no_args(args: &Value) -> Result<(), ModError> {
    match args {
        Value::Null => Ok(()),
        Value::Object(map) if map.is_empty() => Ok(()),
        _ => Err(ModError::bad_args("This call takes no arguments.")),
    }
}

/// The arguments as `T`, whose shape is strict (`deny_unknown_fields`): a field PM doesn't know is
/// refused rather than ignored, so a typo never silently means something else.
pub(super) fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, ModError> {
    T::deserialize(args).map_err(|e| ModError::bad_args(format!("Bad arguments: {e}.")))
}

/// Run the async part of a handler that waits on something outside PM (a model, a website) under
/// [`limits::CALL_TIMEOUT`]: past it the work is dropped (a dropped future's RAII guards run, so a
/// reservation settles and a slot frees) and the mod is told [`Code::CallTimedOut`]. Only
/// cancellable async work stops this way: a `spawn_blocking` step that began is not cancelled, so
/// the handler asks [`super::still_current`] after the await and commits nothing for a call
/// that ended.
pub(super) async fn within_call_timeout<T>(
    work: impl Future<Output = Result<T, ModError>>,
) -> Result<T, ModError> {
    within(limits::CALL_TIMEOUT, work).await
}

/// [`within_call_timeout`] with the limit given (a test's short one).
pub(super) async fn within<T>(
    limit: Duration,
    work: impl Future<Output = Result<T, ModError>>,
) -> Result<T, ModError> {
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or_else(|_elapsed| Err(ModError::call_timed_out()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::Code;
    use serde_json::json;

    #[test]
    fn names_round_trip_and_nothing_else_resolves() {
        for m in Method::ALL {
            assert_eq!(Method::from_name(m.name()), Some(m));
        }
        for name in ["", "theme", "Theme.get", "theme.get ", "storage.delete"] {
            assert_eq!(Method::from_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn theme_get_takes_nothing_and_hands_the_answer_to_the_page() {
        for args in [json!(null), json!({})] {
            assert_eq!(
                theme_get(&args),
                Ok(Reply::Page {
                    action: PageAction::ThemeGet,
                    args: Value::Null
                })
            );
        }
        for args in [json!({"x": 1}), json!([]), json!("dark"), json!(0)] {
            assert_eq!(theme_get(&args).unwrap_err().code, Code::BadArgs, "{args}");
        }
    }

    /// Sets a flag when dropped: the stand-in for a reservation's or an admission's RAII guard.
    struct Guard(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_call_past_its_deadline_is_stopped_told_so_and_its_guards_run() {
        let short = Duration::from_millis(30);
        // Done in time: the answer and an error both pass through untouched.
        assert_eq!(within(short, async { Ok(7) }).await, Ok(7));
        // The helper handlers use is the same under the shipped limit.
        assert_eq!(within_call_timeout(async { Ok(3) }).await, Ok(3));
        let refused: Result<u8, _> = within(short, async {
            Err(ModError::new(Code::NotFound, "nothing"))
        })
        .await;
        assert_eq!(refused.unwrap_err().code, Code::NotFound);

        // Never done: stopped at the deadline, with the one fixed sentence, and the work is dropped
        // (so what it held is given back).
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = Guard(std::sync::Arc::clone(&dropped));
        let began = std::time::Instant::now();
        let stopped: Result<u8, _> = within(short, async move {
            let _held = guard;
            std::future::pending::<()>().await;
            Ok(1)
        })
        .await;
        let e = stopped.unwrap_err();
        assert_eq!(e, ModError::call_timed_out());
        assert_eq!(e.code, Code::CallTimedOut);
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "it waited past the limit"
        );
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the work was not dropped"
        );

        // Work that is slow but inside the limit finishes.
        assert_eq!(
            within(Duration::from_secs(5), async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(2)
            })
            .await,
            Ok(2)
        );
    }

    /// The shipped limit is the one in `limits`, and it outlasts the longest thing a call may
    /// legitimately wait on (a whole fetch; the model route's 20 s slot wait and 120 s cold load).
    #[test]
    fn the_calls_deadline_is_the_limit_and_outlasts_what_a_call_may_wait_for() {
        const {
            assert!(limits::CALL_TIMEOUT.as_secs() > limits::FETCH_TOTAL_TIMEOUT.as_secs());
            assert!(limits::CALL_TIMEOUT.as_secs() >= 20 + 120);
        }
    }

    /// The page performs exactly the actions it knows (`modBridge.ts`'s `PAGE_ACTIONS`), and an
    /// action reaches the page by that name.
    #[test]
    fn every_page_action_is_one_the_page_performs() {
        let page = include_str!("../../../../src/components/mods/modBridge.ts");
        // Prettier wraps the list once it is long, so the declaration is read up to its end.
        let from = page
            .find("export const PAGE_ACTIONS = ")
            .expect("modBridge.ts has PAGE_ACTIONS");
        let line = &page[from..from + page[from..].find("as const;").expect("it ends")];
        for action in PageAction::ALL {
            assert_eq!(
                serde_json::to_value(action).unwrap(),
                json!(action.name()),
                "{action:?}"
            );
            assert!(
                line.contains(&format!("\"{}\"", action.name())),
                "the page doesn't perform {}",
                action.name()
            );
        }
        let listed = line.matches('"').count() / 2;
        assert_eq!(
            listed,
            PageAction::ALL.len(),
            "the page performs more: {line}"
        );
    }
}
