// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `mod_icon`: a mod's icon for the Mods tab, as bytes and a media type the page turns into a
//! `blob:` URL. The main window's CSP has no `pmmod:` in `img-src` (and is not widened for this:
//! the scheme serves only active, consented mods' frames), so the icon is read here and handed over.
//!
//! **What it will read.** One file: the path the installed row's own manifest names as its icon,
//! which the row's file list must list, as a PNG or WebP (an SVG is a document that can carry
//! script, so the tab never shows one) no larger than [`listing::ICON_CAP`]. The path is checked by
//! the verifier's own grammar again (`package::check_path`) and joined onto the mod's version
//! folder, never onto anything the caller named: the command takes a mod id and nothing else.
//!
//! **What it checks first.** The file on disk is a plain file (a link is refused, not followed),
//! has the size the row recorded, and hashes to the sha256 the row recorded, the same bytes the
//! frame would be served; and its first bytes are the signature of the type its extension claims. A
//! file that fails any of these is "no icon", and the row's other state is not touched.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::error::{Code, ModsError, Result};
use super::listing;
use crate::mods::ids::ModId;
use crate::mods::ledger::{Ledger, LedgerStore};
use crate::mods::package;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// An icon for the page: a media type and the bytes, base64 so it travels as an ordinary JSON
/// answer (an icon is under 256 KiB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Icon {
    pub media_type: &'static str,
    pub data: String,
}

/// The media type `bytes` open as, when it agrees with the extension `ext` (lowercase) (pure).
pub fn media_type(ext: &str, bytes: &[u8]) -> Option<&'static str> {
    match ext {
        "png" if bytes.starts_with(PNG) => Some("image/png"),
        "webp" if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" => {
            Some("image/webp")
        }
        _ => None,
    }
}

fn no_icon(why: &str) -> ModsError {
    ModsError::new(
        Code::FileUnreadable,
        format!("It has no icon PM can show ({why})."),
    )
}

/// Read and check the file `rel` under `root`, which the row says is `want_size` bytes hashing to
/// `want_sha` (lowercase hex). Blocking.
pub fn load(root: &Path, rel: &str, want_sha: &str, want_size: u64) -> Result<Icon> {
    if package::check_path(rel).is_err() || want_size > listing::ICON_CAP {
        return Err(no_icon("its path or size isn't allowed"));
    }
    let ext = rel
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| listing::ICON_EXTENSIONS.contains(&e.as_str()))
        .ok_or_else(|| no_icon("it isn't a PNG or WebP"))?;
    let mut full: PathBuf = root.to_path_buf();
    full.extend(rel.split('/'));
    // A link, a folder or a device is not a file PM installed.
    let meta = std::fs::symlink_metadata(&full).map_err(|_| no_icon("the file is missing"))?;
    if !meta.is_file() || meta.len() != want_size {
        return Err(no_icon("the file isn't what PM installed"));
    }
    let mut bytes = Vec::with_capacity(want_size as usize);
    File::open(&full)
        .and_then(|f| f.take(want_size + 1).read_to_end(&mut bytes))
        .map_err(|_| no_icon("the file can't be read"))?;
    if bytes.len() as u64 != want_size || hex::encode(Sha256::digest(&bytes)) != want_sha {
        return Err(no_icon("the file isn't what PM installed"));
    }
    let media_type =
        media_type(&ext, &bytes).ok_or_else(|| no_icon("it isn't the image its name says"))?;
    Ok(Icon {
        media_type,
        data: B64.encode(&bytes),
    })
}

/// `id`'s icon. The icon's path, size and hash come from the installed row, nothing from the
/// caller. Blocking.
pub fn read<S: LedgerStore>(ledger: &Ledger<'_, S>, id: &ModId) -> Result<Icon> {
    let (version, manifest, files) = ledger
        .with_conn(|c| {
            use rusqlite::OptionalExtension;
            Ok(c.query_row(
                "SELECT version, manifest, files FROM mods WHERE id = ?1",
                [id.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?)
        })?
        .ok_or_else(ModsError::not_installed)?;
    let rel = listing::icon_path(listing::read_manifest(&manifest).icon.as_deref(), &files)
        .ok_or_else(|| no_icon("the manifest names none"))?;
    let listed: Vec<Value> = serde_json::from_str(&files).map_err(|_| no_icon("its record"))?;
    let entry = listed
        .iter()
        .find(|f| f["path"].as_str() == Some(rel.as_str()))
        .ok_or_else(|| no_icon("it isn't listed"))?;
    let (sha, size) = (
        entry["sha256"].as_str().unwrap_or_default(),
        entry["size"].as_u64().unwrap_or(u64::MAX),
    );
    let version = crate::mods::disk::VersionName::parse(&version)
        .map_err(|_| no_icon("its version can't name a folder"))?;
    load(&ledger.layout().version_dir(id, &version), &rel, sha, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn png() -> Vec<u8> {
        let mut b = PNG.to_vec();
        b.extend_from_slice(b"rest of the image");
        b
    }

    fn webp() -> Vec<u8> {
        let mut b = b"RIFF\x10\x00\x00\x00WEBPVP8 ".to_vec();
        b.extend_from_slice(b"data");
        b
    }

    fn put(root: &Path, rel: &str, bytes: &[u8]) -> (String, u64) {
        let full = root.join(rel);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, bytes).unwrap();
        (hex::encode(Sha256::digest(bytes)), bytes.len() as u64)
    }

    #[test]
    fn a_png_and_a_webp_are_read_with_their_own_media_type() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, size) = put(dir.path(), "app/icon.png", &png());
        let icon = load(dir.path(), "app/icon.png", &sha, size).unwrap();
        assert_eq!(icon.media_type, "image/png");
        assert_eq!(B64.decode(&icon.data).unwrap(), png());
        let (sha, size) = put(dir.path(), "app/img/icon.WEBP", &webp());
        let icon = load(dir.path(), "app/img/icon.WEBP", &sha, size).unwrap();
        assert_eq!(icon.media_type, "image/webp");
    }

    #[test]
    fn only_png_and_webp_never_svg_jpeg_or_a_file_that_lies_about_its_type() {
        let dir = tempfile::tempdir().unwrap();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        for (rel, bytes) in [
            ("app/icon.svg", svg.to_vec()),
            ("app/icon.jpg", b"\xff\xd8\xff".to_vec()),
            ("app/icon.gif", b"GIF89a".to_vec()),
            ("app/noext", png()),
            // A name that says PNG over bytes that are not.
            ("app/fake.png", svg.to_vec()),
            ("app/fake.webp", png()),
            ("app/short.webp", b"RIFF".to_vec()),
        ] {
            let (sha, size) = put(dir.path(), rel, &bytes);
            let got = load(dir.path(), rel, &sha, size);
            assert_eq!(got.unwrap_err().code, Code::FileUnreadable, "{rel}");
        }
    }

    #[test]
    fn a_file_that_is_not_what_was_installed_is_no_icon() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, size) = put(dir.path(), "app/icon.png", &png());
        // Changed in place, same length: the hash catches it.
        let mut other = png();
        let last = other.len() - 1;
        other[last] ^= 1;
        fs::write(dir.path().join("app/icon.png"), &other).unwrap();
        assert!(load(dir.path(), "app/icon.png", &sha, size).is_err());
        // Another length than recorded.
        fs::write(dir.path().join("app/icon.png"), png()).unwrap();
        assert!(load(dir.path(), "app/icon.png", &sha, size + 1).is_err());
        // Gone.
        fs::remove_file(dir.path().join("app/icon.png")).unwrap();
        assert!(load(dir.path(), "app/icon.png", &sha, size).is_err());
    }

    #[test]
    fn nothing_over_the_cap_is_read_even_if_the_row_says_it_is_that_big() {
        let dir = tempfile::tempdir().unwrap();
        let mut big = PNG.to_vec();
        big.resize(listing::ICON_CAP as usize + 1, 0);
        let (sha, size) = put(dir.path(), "app/icon.png", &big);
        assert!(load(dir.path(), "app/icon.png", &sha, size).is_err());
        let mut at_cap = PNG.to_vec();
        at_cap.resize(listing::ICON_CAP as usize, 0);
        let (sha, size) = put(dir.path(), "app/icon.png", &at_cap);
        assert!(load(dir.path(), "app/icon.png", &sha, size).is_ok());
    }

    #[test]
    fn a_path_that_climbs_or_names_an_absolute_location_is_never_opened() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let (sha, size) = put(outside.path(), "secret.png", &png());
        let climb = format!(
            "app/../../{}/secret.png",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        for rel in [
            climb.as_str(),
            "../secret.png",
            "/etc/passwd.png",
            "app\\..\\secret.png",
            "app//icon.png",
            "app/./icon.png",
            "",
        ] {
            assert!(load(dir.path(), rel, &sha, size).is_err(), "{rel}");
        }
        let _ = outside;
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_icon_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let (sha, size) = put(outside.path(), "real.png", &png());
        fs::create_dir_all(dir.path().join("app")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("real.png"),
            dir.path().join("app/icon.png"),
        )
        .unwrap();
        // Same bytes, same hash, same size: still refused, because it is a link.
        assert!(load(dir.path(), "app/icon.png", &sha, size).is_err());
    }
}
