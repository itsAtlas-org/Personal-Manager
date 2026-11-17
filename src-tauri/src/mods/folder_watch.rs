// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The reload-on-change watcher for loaded test folders (Mods spec §1.15; slice 6, part 4).
//!
//! **A hard inotify budget.** On Linux a watch costs one inotify watch per *directory* out of a
//! machine-wide budget every other app shares (`localfolder.rs`'s watcher says the same), so a
//! loaded folder is watched as exactly two things: its root, **non-recursively** (a change to
//! `manifest.json`, `LICENCE.txt`, `NOTICE.txt`, or the `app/` folder appearing or going), and its
//! `app/` folder **recursively** (the mod's own files, whose subfolders are the mod's, a few). The
//! rest of a developer's folder (`node_modules/`, `.git/`) is never watched. A watch that fails to
//! register is not retried in a loop: it is noted, the folder keeps working, and the Reload command
//! (`reload_test_mod`) is the fallback, so a folder is never held hostage by the budget.
//!
//! **No feedback.** PM's own scan opens and closes the very files it watches, which inotify reports
//! as access events; only creates, modifies and removes count ([`is_change`]), or every reload
//! would cause the next.
//!
//! **Events are hints, not records.** A debounced batch is reduced to the loaded folders it
//! touched ([`owners`]) and each is *rescanned*: nothing is applied from an event's content. A
//! watcher-level error (a queue overflow: events were lost) rescans every loaded folder.
//!
//! The watcher is lazy (no thread until a developer loads a test mod), memory only, and follows the
//! sessions (`ModRuntime::folders`): [`resync`] runs after every change to them and when the store
//! closes, so a watch never outlives its folder's session.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer_opt, DebounceEventResult, Debouncer, RecommendedCache};
use tauri::{AppHandle, Manager};

use super::headers::APP_DIR;
use super::ids::ModId;
use super::package::{LICENCE_FILE, MANIFEST, NOTICE_FILE};
use super::runtime::ModRuntime;

/// How long the folder must be quiet before a batch is reported: an editor's save is several
/// writes, and a build tool writes many files.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// What a watch covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cover {
    /// Only the folder's own entries.
    Entries,
    /// The folder and everything under it.
    Tree,
}

/// The watches wanted for the loaded folders (pure): each root's own entries, and its `app/` folder
/// as a tree when it is a real folder (`app_is_dir`, asked of the disk by the caller).
pub fn plan(
    roots: &[(ModId, PathBuf)],
    app_is_dir: impl Fn(&Path) -> bool,
) -> BTreeMap<PathBuf, Cover> {
    let mut out = BTreeMap::new();
    for (_, root) in roots {
        out.insert(root.clone(), Cover::Entries);
        let app = root.join(APP_DIR.trim_end_matches('/'));
        if app_is_dir(&app) {
            out.insert(app, Cover::Tree);
        }
    }
    out
}

/// What to start and what to stop to get from `have` to `want` (pure). A path whose cover changed
/// is stopped and started again.
pub fn diff(
    have: &BTreeMap<PathBuf, Cover>,
    want: &BTreeMap<PathBuf, Cover>,
) -> (Vec<(PathBuf, Cover)>, Vec<PathBuf>) {
    let start = want
        .iter()
        .filter(|(p, c)| have.get(*p) != Some(*c))
        .map(|(p, c)| (p.clone(), *c))
        .collect();
    let stop = have
        .iter()
        .filter(|(p, c)| want.get(*p) != Some(*c))
        .map(|(p, _)| p.clone())
        .collect();
    (start, stop)
}

/// Whether a change under `root` is one that can alter what the folder serves: the root itself
/// going or coming, `manifest.json`, `LICENCE.txt`, `NOTICE.txt`, or anything under `app/` (pure).
pub fn touches_layout(root: &Path, path: &Path) -> bool {
    if path == root {
        return true;
    }
    let Ok(rest) = path.strip_prefix(root) else {
        return false;
    };
    let Some(first) = rest.components().next() else {
        return true;
    };
    let first = first.as_os_str();
    first == MANIFEST
        || first == LICENCE_FILE
        || first == NOTICE_FILE
        || first == APP_DIR.trim_end_matches('/')
}

/// The loaded folders a batch of changed `paths` touched, in id order (pure). `None` paths (the
/// watcher lost events) name every folder.
pub fn owners(paths: Option<&[PathBuf]>, roots: &[(ModId, PathBuf)]) -> Vec<ModId> {
    let mut out = BTreeSet::new();
    for (id, root) in roots {
        let hit = match paths {
            None => true,
            Some(paths) => paths.iter().any(|p| touches_layout(root, p)),
        };
        if hit {
            out.insert(id.clone());
        }
    }
    out.into_iter().collect()
}

/// Whether a notify event kind is a change to a file or folder, and not PM (or anyone) just
/// reading it (module doc, "No feedback").
pub fn is_change(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

/// Told when loaded folders changed: the paths of the batch, or `None` when events were lost.
pub type OnChange = Arc<dyn Fn(Option<Vec<PathBuf>>) + Send + Sync>;

type Dbc = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

#[derive(Default)]
struct Inner {
    debouncer: Option<Dbc>,
    /// What is registered now.
    watching: BTreeMap<PathBuf, Cover>,
    /// What failed to register, with why, kept until the path is no longer wanted. Never retried
    /// in a loop (module doc).
    failed: BTreeMap<PathBuf, String>,
}

/// The watcher. Managed state; started by the first folder that needs it.
#[derive(Default)]
pub struct FolderWatch {
    inner: Mutex<Inner>,
}

impl FolderWatch {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bring the watches to what `roots` need. `on_change` is given to the debouncer when it is
    /// first needed, and not called again. Returns the paths that could not be watched, with why.
    pub fn sync(
        &self,
        roots: &[(ModId, PathBuf)],
        on_change: impl FnOnce() -> OnChange,
    ) -> BTreeMap<PathBuf, String> {
        let want = plan(roots, |p| {
            std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir())
        });
        let mut inner = self.lock();
        // A failure is forgotten once its path isn't wanted, so the folder loaded again is tried.
        inner.failed.retain(|p, _| want.contains_key(p));
        let (start, stop) = diff(&inner.watching, &want);
        if want.is_empty() && inner.watching.is_empty() {
            return inner.failed.clone();
        }
        for path in stop {
            if let Some(d) = inner.debouncer.as_mut() {
                // Gone already is fine.
                let _ = d.unwatch(&path);
            }
            inner.watching.remove(&path);
        }
        let start: Vec<_> = start
            .into_iter()
            .filter(|(p, _)| !inner.failed.contains_key(p))
            .collect();
        if !start.is_empty() && inner.debouncer.is_none() {
            let notify_me = on_change();
            let handler = move |res: DebounceEventResult| match res {
                Ok(events) => {
                    let paths: Vec<PathBuf> = events
                        .into_iter()
                        .filter(|e| is_change(&e.event.kind))
                        .flat_map(|e| e.event.paths)
                        .collect();
                    if !paths.is_empty() {
                        notify_me(Some(paths));
                    }
                }
                // Events were lost (a queue overflow): rescan everything.
                Err(_) => notify_me(None),
            };
            match new_debouncer_opt::<_, notify::RecommendedWatcher, _>(
                DEBOUNCE,
                None,
                handler,
                RecommendedCache::new(),
                // A link inside a folder is refused by the scan, so it is not followed here either.
                notify::Config::default().with_follow_symlinks(false),
            ) {
                Ok(d) => inner.debouncer = Some(d),
                Err(e) => {
                    for (path, _) in &start {
                        inner
                            .failed
                            .insert(path.clone(), format!("the file watcher didn't start ({e})"));
                    }
                    return inner.failed.clone();
                }
            }
        }
        for (path, cover) in start {
            let Some(d) = inner.debouncer.as_mut() else {
                break;
            };
            let mode = match cover {
                Cover::Entries => RecursiveMode::NonRecursive,
                Cover::Tree => RecursiveMode::Recursive,
            };
            match d.watch(&path, mode) {
                Ok(()) => {
                    inner.watching.insert(path, cover);
                }
                Err(e) => {
                    // A recursive add gives up at the first failure and keeps what it had
                    // registered; hand those back before forgetting the path.
                    let _ = d.unwatch(&path);
                    eprintln!(
                        "mods: {} can't be watched ({e}); on Linux this is usually the machine-wide \
                         inotify limit. The test mod still reloads from its Reload command.",
                        path.display()
                    );
                    inner.failed.insert(path, e.to_string());
                }
            }
        }
        // Nothing left to watch: let the watcher thread go.
        if inner.watching.is_empty() {
            inner.debouncer = None;
        }
        inner.failed.clone()
    }

    /// Whether every watch `root`'s folder needs is registered (the page shows "reloads on change"
    /// only then).
    pub fn watching(&self, root: &Path) -> bool {
        let inner = self.lock();
        inner.watching.contains_key(root) && !inner.failed.keys().any(|p| p.starts_with(root))
    }
}

/// Bring the watches to the loaded folders (after any change to them, and when the store closes).
/// Takes no ledger lock, only the sessions' and the watcher's own, both leaves.
pub fn resync(app: &AppHandle) {
    let (Some(runtime), Some(watch)) = (
        app.try_state::<ModRuntime>(),
        app.try_state::<FolderWatch>(),
    ) else {
        return;
    };
    let roots = runtime.folders().roots();
    let handle = app.clone();
    watch.sync(&roots, move || {
        Arc::new(move |paths| changed(&handle, paths))
    });
}

/// A debounced batch arrived: rescan the folders it touched, on a blocking thread, and say so.
fn changed(app: &AppHandle, paths: Option<Vec<PathBuf>>) {
    let Some(runtime) = app.try_state::<ModRuntime>() else {
        return;
    };
    let ids = owners(paths.as_deref(), &runtime.folders().roots());
    if ids.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for id in ids {
            match super::ledger::reload_test_mod(&app, &id) {
                Ok(_) => {}
                // The reason is on the session ("what happened to the last reload"), which the
                // Developer tab reads; the log has it too.
                Err(e) => eprintln!("mods: test mod {id} didn't reload ({e})"),
            }
        }
        super::refresh::emit_changed(&app, super::refresh::CHANGED_TEST_MOD);
        // A reload may have made `app/` appear or vanish.
        resync(&app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind};
    use std::sync::mpsc;

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    fn roots() -> Vec<(ModId, PathBuf)> {
        vec![
            (id("acme.one"), PathBuf::from("/dev/one")),
            (id("acme.two"), PathBuf::from("/dev/two")),
        ]
    }

    #[test]
    fn a_folder_is_two_watches_its_root_alone_and_its_app_tree() {
        let want = plan(&roots(), |_| true);
        assert_eq!(want.len(), 4);
        assert_eq!(want[Path::new("/dev/one")], Cover::Entries);
        assert_eq!(want[Path::new("/dev/one/app")], Cover::Tree);
        // No app/ folder yet: only the root, which reports app/ appearing.
        let want = plan(&roots(), |_| false);
        assert_eq!(want.len(), 2);
        assert!(want.values().all(|c| *c == Cover::Entries));
        assert!(plan(&[], |_| true).is_empty());
    }

    #[test]
    fn the_diff_starts_what_is_new_and_stops_what_is_gone_or_changed() {
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        let have: BTreeMap<_, _> = [(a.clone(), Cover::Entries), (b.clone(), Cover::Tree)].into();
        let want: BTreeMap<_, _> = [(a.clone(), Cover::Entries)].into();
        assert_eq!(diff(&have, &want), (vec![], vec![b.clone()]));
        assert_eq!(
            diff(&BTreeMap::new(), &want),
            (vec![(a.clone(), Cover::Entries)], vec![])
        );
        // A cover that changed is stopped and started again.
        let want: BTreeMap<_, _> = [(a.clone(), Cover::Tree), (b.clone(), Cover::Tree)].into();
        assert_eq!(
            diff(&have, &want),
            (vec![(a.clone(), Cover::Tree)], vec![a])
        );
        assert_eq!(diff(&have, &have), (vec![], vec![]));
    }

    #[test]
    fn only_the_layouts_files_and_app_count_as_the_folders_own_change() {
        let root = Path::new("/dev/one");
        for path in [
            "/dev/one",
            "/dev/one/manifest.json",
            "/dev/one/LICENCE.txt",
            "/dev/one/NOTICE.txt",
            "/dev/one/app",
            "/dev/one/app/main.js",
            "/dev/one/app/deep/er/x.css",
        ] {
            assert!(touches_layout(root, Path::new(path)), "{path}");
        }
        for path in [
            "/dev/one/README.md",
            "/dev/one/node_modules/x/index.js",
            "/dev/one/.git/HEAD",
            "/dev/one/package.json",
            "/dev/one/pmmod/checksums.json",
            // A sibling that merely starts with the same text, and another folder.
            "/dev/one2/manifest.json",
            "/dev/two/manifest.json",
            "/dev/manifest.json",
            "/dev/one/appendix/x",
        ] {
            assert!(!touches_layout(root, Path::new(path)), "{path}");
        }
    }

    #[test]
    fn a_batch_names_the_folders_it_touched_and_lost_events_name_them_all() {
        let paths = [
            PathBuf::from("/dev/two/app/main.js"),
            PathBuf::from("/dev/x"),
        ];
        assert_eq!(owners(Some(&paths), &roots()), vec![id("acme.two")]);
        let both = [
            PathBuf::from("/dev/one/manifest.json"),
            PathBuf::from("/dev/two/app/a.js"),
        ];
        assert_eq!(
            owners(Some(&both), &roots()),
            vec![id("acme.one"), id("acme.two")]
        );
        assert!(owners(Some(&[PathBuf::from("/elsewhere")]), &roots()).is_empty());
        assert!(owners(Some(&[]), &roots()).is_empty());
        assert_eq!(owners(None, &roots()), vec![id("acme.one"), id("acme.two")]);
        assert!(owners(None, &[]).is_empty());
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        assert!(is_change(&EventKind::Create(CreateKind::File)));
        assert!(is_change(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_change(&EventKind::Remove(RemoveKind::File)));
        assert!(is_change(&EventKind::Any));
        // PM's own scan opens and closes the files it watches.
        assert!(!is_change(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!is_change(&EventKind::Other));
    }

    /// The real watcher over a real folder: an edit under `app/` and a new `manifest.json` are
    /// reported, reading is not, a folder that is no longer loaded is not watched, and a failed
    /// registration is noted rather than retried.
    #[test]
    fn a_real_edit_is_reported_and_a_folder_let_go_is_not_watched_any_more() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/main.js"), "a").unwrap();
        std::fs::write(root.join("manifest.json"), "{}").unwrap();
        let watch = FolderWatch::default();
        let (tx, rx) = mpsc::channel::<Option<Vec<PathBuf>>>();
        let tx = Mutex::new(tx);
        let loaded = vec![(id("acme.one"), root.clone())];
        let failed = watch.sync(&loaded, || {
            Arc::new(move |paths| {
                let _ = tx.lock().unwrap().send(paths);
            })
        });
        assert!(failed.is_empty(), "{failed:?}");
        assert!(watch.watching(&root));

        // Reading is silent.
        let _ = std::fs::read(root.join("app/main.js")).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err());

        // A write under app/ comes through.
        std::fs::write(root.join("app/main.js"), "b").unwrap();
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("an edit under app/ is reported")
            .unwrap();
        assert!(got.iter().any(|p| p.ends_with("app/main.js")), "{got:?}");

        // The root's own entries: manifest.json.
        std::fs::write(root.join("manifest.json"), "{\"a\":1}").unwrap();
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a manifest edit is reported")
            .unwrap();
        assert!(got.iter().any(|p| p.ends_with("manifest.json")), "{got:?}");

        // A file the layout doesn't include is not the root's concern: the watch on the root sees
        // it, and the owner test drops it.
        std::fs::write(root.join("README.md"), "x").unwrap();
        if let Ok(Some(paths)) = rx.recv_timeout(Duration::from_secs(3)) {
            assert!(owners(Some(&paths), &loaded).is_empty(), "{paths:?}");
        }

        // Unloaded: nothing is watched, so nothing is reported.
        watch.sync(&[], || unreachable!("no watcher is made for nothing"));
        assert!(!watch.watching(&root));
        std::fs::write(root.join("app/main.js"), "c").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err());
    }

    #[test]
    fn a_watch_that_cannot_be_made_is_noted_and_not_retried_until_the_folder_is_loaded_again() {
        let watch = FolderWatch::default();
        let nowhere = PathBuf::from("/this/folder/does/not/exist");
        let loaded = vec![(id("acme.one"), nowhere.clone())];
        let failed = watch.sync(&loaded, || Arc::new(|_| {}));
        assert!(failed.contains_key(&nowhere), "{failed:?}");
        assert!(!watch.watching(&nowhere));
        // The second sync doesn't try it again (the closure is never needed, and the note stays).
        let again = watch.sync(&loaded, || unreachable!("not retried"));
        assert_eq!(again.keys().collect::<Vec<_>>(), vec![&nowhere]);
        // Unloaded, the note goes, so the same folder loaded again is tried.
        assert!(watch.sync(&[], || unreachable!()).is_empty());
    }
}
