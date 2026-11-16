// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A developer's unpacked test folder (Mods spec §1.15, plan decision 8; slice 6, part 4): reading it
//! into a [`VerifiedPackage`], and the session record of which folders are loaded.
//!
//! **The folder is the developer's and PM only reads it.** Nothing is copied into PM's mods folder,
//! nothing is ever written to or deleted from the folder, and the served files are the folder's own,
//! read through the same `pmmod` scheme and the same per-serve hash check as an installed mod
//! (`runtime::read_verified`), so a file edited after the scan is refused until the reload reads it
//! again.
//!
//! **What [`scan`] reads** is the package layout and nothing else: `manifest.json`, `LICENCE.txt`,
//! `NOTICE.txt` (when present) and everything under `app/`. Other files in the folder (a README, a
//! `package.json`, `node_modules/`, `.git/`, an unpacked archive's `pmmod/`) are never opened.
//! Everything it does read is held to the verifier's rules: every name through
//! [`package::check_path`] (a character outside the mod set refuses the whole folder, naming the file,
//! as it would refuse the packed file), no symlink, device or FIFO anywhere in what is read (a link
//! out of `app/` is how a folder would serve some other file of the user's), no two names equal
//! after case-folding, and the verifier's caps on entries, one file and the whole, enforced while
//! reading. The manifest and licence then go through [`package::verify_folder`].
//!
//! **A loaded folder is a session** ([`FolderSessions`]): its canonical root, the registry entry
//! built from the last good scan, and the last reload's problem. It lives in memory only, owned by
//! the registry (`ModRuntime::folders`), and is forgotten when the store closes, the mod is
//! uninstalled or unloaded, or developer mode goes off. The row in `mods` that goes with it
//! (`installed_from = 'folder'`) is deleted by the same events and, for a crash, by every writer's
//! open (`ledger/folder.rs`), and a startup never serves a folder row that has no session.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use sha2::{Digest, Sha256};

use super::headers::APP_DIR;
use super::ids::ModId;
use super::package::{
    self, Limits, PackageError, Refusal, VerifiedFile, VerifiedPackage, LICENCE_FILE, MANIFEST,
    MANIFEST_CAP, NOTICE_FILE,
};
use super::runtime::InstalledMod;

/// The folder, read: its canonical root and the files that passed.
#[derive(Debug, Clone)]
pub struct Scan {
    /// Canonical, so the registry's containment check compares like with like.
    pub root: PathBuf,
    pub pkg: VerifiedPackage,
}

fn refuse(refusal: Refusal, detail: impl Into<String>) -> PackageError {
    PackageError {
        refusal,
        detail: detail.into(),
    }
}

/// `scan_with` under the verifier's default caps.
pub fn scan(dir: &Path, allow_atlas: bool) -> Result<Scan, PackageError> {
    scan_with(dir, allow_atlas, &Limits::DEFAULT)
}

/// Read the folder at `dir` (module doc). `allow_atlas`: a development build, where an `atlas.`
/// folder may be loaded ([`package::verify_folder`]). Blocking file I/O: call from a blocking thread,
/// never under a lock.
pub fn scan_with(dir: &Path, allow_atlas: bool, limits: &Limits) -> Result<Scan, PackageError> {
    let root = fs::canonicalize(dir).map_err(|e| {
        refuse(
            Refusal::NotAFile,
            format!("that folder can't be opened ({e})"),
        )
    })?;
    if !fs::metadata(&root).is_ok_and(|m| m.is_dir()) {
        return Err(refuse(Refusal::NotAFile, "that isn't a folder"));
    }
    let mut reading = Reading {
        files: BTreeMap::new(),
        entries: 0,
        left: limits.total,
        limits: *limits,
    };
    for name in [MANIFEST, LICENCE_FILE, NOTICE_FILE] {
        let cap = if name == MANIFEST {
            MANIFEST_CAP
        } else {
            limits.entry
        };
        reading.file(&root.join(name), name, cap)?;
    }
    let app = root.join(APP_DIR.trim_end_matches('/'));
    match fs::symlink_metadata(&app) {
        Ok(m) if m.file_type().is_dir() => reading.walk(&app, APP_DIR.trim_end_matches('/'))?,
        Ok(_) => {
            return Err(refuse(
                Refusal::NotAFile,
                "app isn't a plain folder (a link, or a file)",
            ))
        }
        // No app/ folder: the manifest's entry page then isn't one of its files, and says so.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(refuse(
                Refusal::Corrupt,
                format!("its app folder can't be read ({e})"),
            ))
        }
    }
    let names: Vec<String> = reading.files.keys().cloned().collect();
    package::check_case_collisions(&names)?;
    let pkg = package::verify_folder(reading.files, allow_atlas)?;
    Ok(Scan { root, pkg })
}

/// The files read so far and what is left of the caps.
struct Reading {
    files: BTreeMap<String, VerifiedFile>,
    entries: usize,
    left: usize,
    limits: Limits,
}

impl Reading {
    /// One entry counted against the cap on how many a folder may hold.
    fn count(&mut self) -> Result<(), PackageError> {
        self.entries += 1;
        if self.entries > self.limits.entries {
            return Err(refuse(Refusal::TooManyEntries, "it holds too many files"));
        }
        Ok(())
    }

    /// A file of the layout at `path` (named `name` in the package): absent is fine, a link or
    /// anything but a plain file is refused, and what is read is capped at `cap`.
    fn file(&mut self, path: &Path, name: &str, cap: usize) -> Result<(), PackageError> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(refuse(
                    Refusal::Corrupt,
                    format!("{} can't be read ({e})", package::quote(name)),
                ))
            }
        };
        self.count()?;
        if !meta.file_type().is_file() {
            return Err(refuse(
                Refusal::NotAFile,
                format!("{} isn't a plain file", package::quote(name)),
            ));
        }
        let cap = cap.min(self.limits.entry);
        let limit = cap.min(self.left);
        let too_large = || {
            if limit < cap {
                refuse(Refusal::TotalTooLarge, "its files are too large together")
            } else {
                refuse(
                    Refusal::EntryTooLarge,
                    format!("{} is too large", package::quote(name)),
                )
            }
        };
        // Declared size first, so a huge file is refused without being opened.
        if meta.len() > limit as u64 {
            return Err(too_large());
        }
        let corrupt = |e: io::Error| {
            refuse(
                Refusal::Corrupt,
                format!("{} couldn't be read ({e})", package::quote(name)),
            )
        };
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(corrupt)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(corrupt)?;
        // The file grew while it was read: the same refusal as a lying declared size.
        if bytes.len() > limit {
            return Err(too_large());
        }
        self.left -= bytes.len();
        let sha256: [u8; 32] = Sha256::digest(&bytes).into();
        self.files
            .insert(name.to_owned(), VerifiedFile { sha256, bytes });
        Ok(())
    }

    /// Everything under `dir` (named `prefix` in the package), depth first. Every name must be a
    /// plain UTF-8 name that [`package::check_path`] accepts, and every entry a plain file or a plain
    /// folder.
    fn walk(&mut self, dir: &Path, prefix: &str) -> Result<(), PackageError> {
        let listing = fs::read_dir(dir).map_err(|e| {
            refuse(
                Refusal::Corrupt,
                format!("{} can't be listed ({e})", package::quote(prefix)),
            )
        })?;
        let mut children = Vec::new();
        for entry in listing {
            let entry = entry.map_err(|e| {
                refuse(
                    Refusal::Corrupt,
                    format!("{} can't be listed ({e})", package::quote(prefix)),
                )
            })?;
            children.push(entry);
        }
        // Sorted, so the first refusal is the same one every time.
        children.sort_by_key(fs::DirEntry::file_name);
        for entry in children {
            let raw = entry.file_name();
            let Some(name) = raw.to_str() else {
                return Err(refuse(
                    Refusal::EntryName,
                    format!(
                        "{} isn't a plain file name",
                        package::quote(&raw.to_string_lossy())
                    ),
                ));
            };
            let path = format!("{prefix}/{name}");
            package::check_path(&path)?;
            let full = entry.path();
            let kind = fs::symlink_metadata(&full)
                .map_err(|e| {
                    refuse(
                        Refusal::Corrupt,
                        format!("{} can't be read ({e})", package::quote(&path)),
                    )
                })?
                .file_type();
            if kind.is_dir() {
                self.count()?;
                self.walk(&full, &path)?;
            } else {
                // A file reads as itself; a link, device, FIFO or socket is refused by `file`.
                self.file(&full, &path, self.limits.entry)?;
            }
        }
        Ok(())
    }
}

/// What the registry serves for a scan: the folder's own files, hashed as read.
pub fn served(scan: &Scan) -> io::Result<InstalledMod> {
    InstalledMod::from_package(&scan.pkg, &scan.root)
}

// ---- The sessions ------------------------------------------------------------------------------

/// One loaded folder.
#[derive(Debug, Clone)]
pub struct FolderSource {
    /// The folder, canonical.
    pub root: PathBuf,
    /// What the registry serves for the last good scan.
    pub served: InstalledMod,
    /// Why the last reload didn't take, in words, until one does.
    pub problem: Option<String>,
}

/// The loaded folders, by mod id. Owned by the registry (`ModRuntime::folders`); memory only.
#[derive(Default)]
pub struct FolderSessions {
    inner: Mutex<BTreeMap<ModId, FolderSource>>,
}

impl FolderSessions {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<ModId, FolderSource>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start (or restart) a session: this folder, serving this.
    pub fn set(&self, id: ModId, root: PathBuf, served: InstalledMod) {
        self.lock().insert(
            id,
            FolderSource {
                root,
                served,
                problem: None,
            },
        );
    }

    /// A reload that took: what is served now, and no problem.
    pub fn update(&self, id: &ModId, served: InstalledMod) {
        if let Some(s) = self.lock().get_mut(id) {
            s.served = served;
            s.problem = None;
        }
    }

    pub fn set_problem(&self, id: &ModId, problem: Option<String>) {
        if let Some(s) = self.lock().get_mut(id) {
            s.problem = problem;
        }
    }

    /// What the registry serves for `id`, when a session holds it.
    pub fn served(&self, id: &ModId) -> Option<InstalledMod> {
        self.lock().get(id).map(|s| s.served.clone())
    }

    pub fn get(&self, id: &ModId) -> Option<FolderSource> {
        self.lock().get(id).cloned()
    }

    #[cfg(test)]
    pub fn contains(&self, id: &ModId) -> bool {
        self.lock().contains_key(id)
    }

    /// Every loaded folder, id and root.
    pub fn roots(&self) -> Vec<(ModId, PathBuf)> {
        self.lock()
            .iter()
            .map(|(id, s)| (id.clone(), s.root.clone()))
            .collect()
    }

    pub fn forget(&self, id: &ModId) {
        self.lock().remove(id);
    }

    /// Forget every session: the store closed, or developer mode went off.
    pub fn forget_all(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mods::package::tests::{signed_package, unsigned_package};
    use serde_json::{json, Value};

    /// Write `files` under `dir`, as a developer's folder holds them.
    pub(crate) fn write_files(dir: &Path, files: &BTreeMap<String, VerifiedFile>) {
        for (path, f) in files {
            let full = dir.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, &f.bytes).unwrap();
        }
    }

    /// The fixture third-party mod (`acme.tool`) as a folder.
    pub(crate) fn tool_folder(
        dir: &Path,
        change: impl FnOnce(&mut Value, &mut BTreeMap<String, Vec<u8>>),
    ) {
        let bytes = unsigned_package(change);
        write_files(dir, &package::verify_unsigned(&bytes).unwrap().files);
    }

    /// Rewrite the folder's `manifest.json` after `change`: for a manifest the packer's own checks
    /// would refuse, which `tool_folder` therefore can't make.
    pub(crate) fn edit_manifest(dir: &Path, change: impl FnOnce(&mut Value)) {
        let path = dir.join("manifest.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut manifest);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }

    fn refusal(r: Result<Scan, PackageError>) -> Refusal {
        match r {
            Ok(_) => panic!("the folder was accepted"),
            Err(e) => e.refusal,
        }
    }

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        tool_folder(dir.path(), |_, _| {});
        dir
    }

    #[test]
    fn a_well_formed_folder_scans_to_its_own_files_hashed_as_read() {
        let dir = folder();
        let scan = scan(dir.path(), false).unwrap();
        assert_eq!(scan.root, fs::canonicalize(dir.path()).unwrap());
        assert!(!scan.pkg.is_signed());
        assert_eq!(scan.pkg.manifest.id.as_str(), "acme.tool");
        let paths: Vec<&str> = scan.pkg.files.keys().map(String::as_str).collect();
        assert!(paths.contains(&"manifest.json") && paths.contains(&"app/index.html"));
        for (path, f) in &scan.pkg.files {
            assert_eq!(f.bytes, fs::read(dir.path().join(path)).unwrap(), "{path}");
            assert_eq!(f.sha256, <[u8; 32]>::from(Sha256::digest(&f.bytes)));
        }
        // It serves from the folder itself.
        assert!(served(&scan).is_ok());
    }

    #[test]
    fn only_the_package_layout_is_read_and_the_rest_of_the_folder_is_never_opened() {
        let dir = folder();
        let p = dir.path();
        // A developer's own clutter, one of it unreadable names and one a link that would escape.
        fs::write(p.join("README.md"), "hello").unwrap();
        fs::write(p.join("package.json"), "{}").unwrap();
        fs::create_dir_all(p.join("node_modules/x~y")).unwrap();
        fs::write(p.join("node_modules/x~y/index.js"), "x").unwrap();
        fs::create_dir_all(p.join("pmmod")).unwrap();
        fs::write(p.join("pmmod/checksums.json"), "{}").unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", p.join("secrets-link")).unwrap();
        let scan = scan(p, false).unwrap();
        for path in scan.pkg.files.keys() {
            assert!(
                path == "manifest.json"
                    || path == "LICENCE.txt"
                    || path == "NOTICE.txt"
                    || path.starts_with("app/"),
                "{path} was read"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_anywhere_in_what_is_read_refuses_the_folder() {
        use std::os::unix::fs::symlink;
        // A file link inside app/ (to a file of the user's that isn't the mod's).
        let dir = folder();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(
            outside.path().join("secret.txt"),
            dir.path().join("app/secret.txt"),
        )
        .unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::NotAFile);
        // A folder link inside app/, so its files would be read through it.
        let dir = folder();
        symlink(outside.path(), dir.path().join("app/data")).unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::NotAFile);
        // app/ itself a link.
        let dir = folder();
        let real = dir.path().join("elsewhere");
        fs::rename(dir.path().join("app"), &real).unwrap();
        symlink(&real, dir.path().join("app")).unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::NotAFile);
        // manifest.json, LICENCE.txt and NOTICE.txt links.
        for name in ["manifest.json", "LICENCE.txt", "NOTICE.txt"] {
            let dir = folder();
            let target = outside.path().join(format!("{name}.real"));
            fs::copy(dir.path().join(name), &target).unwrap();
            fs::remove_file(dir.path().join(name)).unwrap();
            symlink(&target, dir.path().join(name)).unwrap();
            assert_eq!(
                refusal(scan(dir.path(), false)),
                Refusal::NotAFile,
                "{name}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_app_is_refused_and_never_opened() {
        let dir = folder();
        let fifo = dir.path().join("app/pipe");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !status.is_ok_and(|s| s.success()) {
            return; // no mkfifo here
        }
        // Opening it would block forever: the test returning at all is the proof it isn't opened.
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::NotAFile);
    }

    #[test]
    fn a_name_the_packer_would_refuse_refuses_the_folder_and_names_the_file() {
        for bad in [
            "app/main.js~",
            "app/a b.js",
            "app/Ünï.js",
            "app/CON.js",
            "app/x:y",
        ] {
            let dir = folder();
            // `x:y` can't be created on Windows; skip a name this filesystem won't take.
            if fs::write(dir.path().join(bad), "x").is_err() {
                continue;
            }
            let e = scan(dir.path(), false).unwrap_err();
            assert_eq!(e.refusal, Refusal::EntryName, "{bad}");
            assert!(e.detail.contains("app/"), "{bad}: {}", e.detail);
        }
    }

    #[test]
    fn two_names_the_same_after_case_folding_refuse_the_folder() {
        let dir = folder();
        fs::write(dir.path().join("app/Extra.js"), "a").unwrap();
        fs::write(dir.path().join("app/extra.js"), "b").unwrap();
        // A case-insensitive filesystem has made one file of the two: nothing to refuse there.
        if fs::read_dir(dir.path().join("app"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("extra.js")
            })
            .count()
            == 2
        {
            assert_eq!(refusal(scan(dir.path(), false)), Refusal::CaseCollision);
        }
    }

    #[test]
    fn the_verifiers_caps_hold_on_what_is_read() {
        let dir = folder();
        let small = Limits {
            file: 1 << 20,
            entries: 3,
            entry: 1 << 20,
            total: 1 << 20,
        };
        // manifest, licence, notice and the app files are more than three entries.
        assert_eq!(
            scan_with(dir.path(), false, &small).unwrap_err().refusal,
            Refusal::TooManyEntries
        );
        // One file over the per-file cap, by its declared size.
        let big_entry = Limits {
            entry: 8,
            ..Limits::DEFAULT
        };
        assert_eq!(
            scan_with(dir.path(), false, &big_entry)
                .unwrap_err()
                .refusal,
            Refusal::EntryTooLarge
        );
        // Every file fine, together too much.
        let sizes: usize = fs::read_dir(dir.path().join("app"))
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len() as usize)
            .sum();
        let tight = Limits {
            total: sizes,
            ..Limits::DEFAULT
        };
        assert_eq!(
            scan_with(dir.path(), false, &tight).unwrap_err().refusal,
            Refusal::TotalTooLarge
        );
        // The manifest has its own, smaller, cap.
        let dir = folder();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(dir.path().join("manifest.json")).unwrap()).unwrap();
        manifest["description"] = json!("x".repeat(MANIFEST_CAP));
        fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert_eq!(
            refusal(scan(dir.path(), false)),
            Refusal::EntryTooLarge,
            "a manifest over its cap is refused before it is parsed"
        );
    }

    #[test]
    fn what_is_not_a_folder_or_has_no_manifest_or_licence_is_refused() {
        let dir = folder();
        assert_eq!(
            refusal(scan(&dir.path().join("missing"), false)),
            Refusal::NotAFile
        );
        assert_eq!(
            refusal(scan(&dir.path().join("manifest.json"), false)),
            Refusal::NotAFile
        );
        fs::remove_file(dir.path().join("manifest.json")).unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Missing);
        let dir = folder();
        fs::remove_file(dir.path().join("LICENCE.txt")).unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Licence);
        let dir = folder();
        fs::write(dir.path().join("LICENCE.txt"), "MIT").unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Licence);
        // No app/ folder: the manifest's entry page isn't one of its files.
        let dir = folder();
        fs::remove_dir_all(dir.path().join("app")).unwrap();
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Manifest);
    }

    #[test]
    fn the_manifest_is_judged_like_a_files_and_the_tier_rules_are_the_folders() {
        // A bad manifest field is refused as it is in a packed file.
        let dir = folder();
        edit_manifest(dir.path(), |m| m["version"] = json!("1.0"));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Manifest);
        // An unknown field, and a licence that isn't the AGPL.
        let dir = folder();
        edit_manifest(dir.path(), |m| m["surprise"] = json!(true));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Manifest);
        let dir = folder();
        edit_manifest(dir.path(), |m| m["licence"] = json!("MIT"));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Licence);
        // Second party and an atlas. id with third party: refused.
        let dir = folder();
        edit_manifest(dir.path(), |m| m["publisher"]["tier"] = json!("second"));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Manifest);
        let dir = folder();
        edit_manifest(dir.path(), |m| m["id"] = json!("atlas.thing"));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Id);
        // First party outside atlas.: refused.
        let dir = folder();
        edit_manifest(dir.path(), |m| m["publisher"]["tier"] = json!("first"));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Manifest);
        // A path the manifest names that isn't one of the folder's app/ files.
        let dir = folder();
        edit_manifest(dir.path(), |m| m["entry"]["page"] = json!("app/gone.html"));
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Manifest);
    }

    #[test]
    fn an_atlas_folder_loads_only_where_the_caller_allows_it_and_says_first_party() {
        let (bytes, key) = signed_package(|_, _| {});
        let pkg = package::verify(&bytes, &[key]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_files(dir.path(), &pkg.files);
        assert_eq!(pkg.manifest.id.as_str(), "atlas.map");
        // A release build: never.
        assert_eq!(refusal(scan(dir.path(), false)), Refusal::Id);
        // A development build: yes, when the manifest says first party.
        let scan_dev = scan(dir.path(), true).unwrap();
        assert_eq!(scan_dev.pkg.manifest.id.as_str(), "atlas.map");
        assert!(!scan_dev.pkg.is_signed());
        // ... and not for an atlas. id that claims to be third party.
        let (bytes, key) = signed_package(|m, _| m["publisher"]["tier"] = json!("second"));
        // The signed verifier refuses that manifest (its id is atlas.), so build the folder by hand
        // from the verified fixture and change the claim.
        assert!(package::verify(&bytes, &[key]).is_err());
        let mut files = pkg.files.clone();
        let mut manifest: Value = serde_json::from_slice(&files[MANIFEST].bytes).unwrap();
        manifest["publisher"]["tier"] = json!("third");
        let edited = serde_json::to_vec(&manifest).unwrap();
        files.insert(
            MANIFEST.to_owned(),
            VerifiedFile {
                sha256: Sha256::digest(&edited).into(),
                bytes: edited,
            },
        );
        let dir = tempfile::tempdir().unwrap();
        write_files(dir.path(), &files);
        assert_eq!(refusal(scan(dir.path(), true)), Refusal::Manifest);
    }

    #[test]
    fn a_file_edited_after_the_scan_is_no_longer_what_was_hashed() {
        // The serve-time check is the registry's (`runtime::read_verified`); what this pins is that
        // the scan's hash is of the bytes read, so any later edit differs from it.
        let dir = folder();
        let first = scan(dir.path(), false).unwrap();
        fs::write(dir.path().join("app/main.js"), "console.log('edited');").unwrap();
        let second = scan(dir.path(), false).unwrap();
        assert_ne!(
            first.pkg.files["app/main.js"].sha256,
            second.pkg.files["app/main.js"].sha256
        );
        assert_ne!(served(&first).unwrap(), served(&second).unwrap());
    }

    #[test]
    fn sessions_hold_a_folder_until_it_is_forgotten_one_or_all() {
        let dir = folder();
        let scan = scan(dir.path(), false).unwrap();
        let id = scan.pkg.manifest.id.clone();
        let other = ModId::parse("acme.other").unwrap();
        let sessions = FolderSessions::default();
        assert!(sessions.served(&id).is_none() && sessions.roots().is_empty());
        sessions.set(id.clone(), scan.root.clone(), served(&scan).unwrap());
        assert!(sessions.contains(&id) && !sessions.contains(&other));
        assert_eq!(sessions.roots(), vec![(id.clone(), scan.root.clone())]);
        // A reload's problem sticks until a reload takes.
        sessions.set_problem(&id, Some("it is half written".into()));
        assert_eq!(
            sessions.get(&id).unwrap().problem.as_deref(),
            Some("it is half written")
        );
        sessions.update(&id, served(&scan).unwrap());
        assert!(sessions.get(&id).unwrap().problem.is_none());
        // Naming a mod that has no session changes nothing.
        sessions.set_problem(&other, Some("x".into()));
        sessions.update(&other, served(&scan).unwrap());
        assert!(!sessions.contains(&other));
        sessions.forget(&other);
        assert!(sessions.contains(&id));
        sessions.forget(&id);
        assert!(!sessions.contains(&id));
        sessions.set(id.clone(), scan.root.clone(), served(&scan).unwrap());
        sessions.forget_all();
        assert!(sessions.roots().is_empty());
    }
}
