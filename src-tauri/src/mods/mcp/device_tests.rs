// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! This computer's MCP file: what it holds, what makes it "not ours" (off for every vault), the 0600
//! atomic write, the caps, and forgetting.

use std::path::Path;

use super::device::*;
use crate::mods::devmode::Here;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn here(dir: &Path, vault: &str) -> Here {
    Here::new(A, vault, dir)
}

fn h(c: char) -> String {
    c.to_string().repeat(64)
}

fn pc(id: i64, c: char) -> PairedClient {
    PairedClient {
        id,
        hash: h(c),
        scopes_hash: h('b'),
    }
}

fn put(dir: &Path, text: &str) {
    std::fs::write(dir.join(DEVICE_FILE), text).unwrap();
}

fn text(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(DEVICE_FILE)).unwrap()
}

/// A file PM would write, for `here`'s vault, on, with one pair.
fn good(here: &Here) -> String {
    format!(
        r#"{{"v":1,"vaults":[{{"id":"{}","at":"{}","on":true,"clients":[{{"id":1,"hash":"{}","scopes_hash":"{}"}}]}}]}}"#,
        here.vault(),
        here.place(),
        h('a'),
        h('b'),
    )
}

#[test]
fn a_good_file_reads_back_and_nothing_else_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    assert_eq!(device_state(&me), DeviceState::default());
    put(dir.path(), &good(&me));
    let state = device_state(&me);
    assert!(state.on);
    assert_eq!(state.clients, vec![pc(1, 'a')]);
    // Another vault of this device is off.
    assert_eq!(
        device_state(&here(dir.path(), "vault-two")),
        DeviceState::default()
    );
}

#[test]
fn a_file_that_is_not_ours_is_off_for_every_vault() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    let ok = good(&me);
    let entry = |extra: &str| {
        format!(
            r#"{{"v":1,"vaults":[{{"id":"vault-one","at":"{}","on":true,{extra}"clients":[]}}]}}"#,
            me.place()
        )
    };
    let seventeen: Vec<String> = (1..=17)
        .map(|i| {
            format!(
                r#"{{"id":{i},"hash":"{}","scopes_hash":"{}"}}"#,
                h('a'),
                h('b')
            )
        })
        .collect();
    let too_many_clients = format!(
        r#"{{"v":1,"vaults":[{{"id":"vault-one","at":"{}","on":true,"clients":[{}]}}]}}"#,
        me.place(),
        seventeen.join(",")
    );
    let many_vaults = format!(
        r#"{{"v":1,"vaults":[{}]}}"#,
        (0..=MAX_VAULTS)
            .map(|i| format!(r#"{{"id":"vault-{i}","at":"x","on":true,"clients":[]}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let junk: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("not json", "not json".into()),
        ("an array", "[]".into()),
        ("wrong version", ok.replace(r#""v":1"#, r#""v":2"#)),
        ("a field it does not know", entry(r#""extra":1,"#)),
        ("a short hash", ok.replace(&h('a'), &"a".repeat(63))),
        ("an upper-case hash", ok.replace(&h('a'), &"A".repeat(64))),
        ("a pair with id 0", ok.replace(r#""id":1,"#, r#""id":0,"#)),
        ("too many clients", too_many_clients),
        ("too many vaults", many_vaults),
        (
            "a vault twice",
            format!(
                r#"{{"v":1,"vaults":[{e},{e}]}}"#,
                e = format!(
                    r#"{{"id":"vault-one","at":"{}","on":true,"clients":[]}}"#,
                    me.place()
                )
            ),
        ),
        (
            "an app twice",
            ok.replace(
                r#""clients":["#,
                &format!(
                    r#""clients":[{{"id":1,"hash":"{}","scopes_hash":"{}"}},"#,
                    h('c'),
                    h('d')
                ),
            ),
        ),
        (
            "an empty vault id",
            ok.replace(&format!(r#""id":"{}""#, me.vault()), r#""id":"""#),
        ),
    ];
    for (why, file) in junk {
        put(dir.path(), &file);
        assert_eq!(device_state(&me), DeviceState::default(), "{why}");
    }
    // Over the size cap: valid JSON, one extra byte of room too many.
    let padded = format!("{}{}", ok, " ".repeat(MAX_FILE_BYTES as usize));
    put(dir.path(), &padded);
    assert_eq!(device_state(&me), DeviceState::default(), "oversize");
    // And the good one still reads, so none of the above was a false pass.
    put(dir.path(), &ok);
    assert!(device_state(&me).on);
}

#[test]
fn a_write_replaces_a_file_that_is_not_ours_and_makes_it_ours() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    put(dir.path(), "not json at all");
    set_on(&me, true).unwrap();
    assert!(device_state(&me).on);
    assert!(text(dir.path()).starts_with(r#"{"v":1,"vaults":["#));
}

#[cfg(unix)]
#[test]
fn the_file_is_written_0600_and_no_temp_file_is_left() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    set_on(&me, true).unwrap();
    pair(&me, pc(1, 'a')).unwrap();
    unpair(&me, 1).unwrap();
    let mode = std::fs::metadata(dir.path().join(DEVICE_FILE))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, [DEVICE_FILE], "a temp file was left behind");
}

#[test]
fn pairing_off_and_on_keep_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    pair(&me, pc(1, 'a')).unwrap();
    // Pairing alone does not switch the server on.
    assert!(!device_state(&me).on);
    set_on(&me, true).unwrap();
    pair(&me, pc(2, 'c')).unwrap();
    assert_eq!(device_state(&me).clients.len(), 2);
    // Off keeps the apps; on again needs no new token.
    set_on(&me, false).unwrap();
    let off = device_state(&me);
    assert!(!off.on);
    assert_eq!(off.clients.len(), 2);
    // Pairing the same app again replaces its pair, not adds one.
    pair(&me, pc(1, 'd')).unwrap();
    assert_eq!(device_state(&me).clients.len(), 2);
    assert!(device_state(&me).clients.iter().any(|c| c.hash == h('d')));
    unpair(&me, 1).unwrap();
    unpair(&me, 1).unwrap();
    assert_eq!(device_state(&me).clients.len(), 1);
    // A pair the file would reject is refused up front.
    assert!(pair(
        &me,
        PairedClient {
            id: 3,
            hash: "x".into(),
            scopes_hash: h('b')
        }
    )
    .is_err());
    assert!(pair(&me, pc(0, 'a')).is_err());
}

#[test]
fn switching_off_in_a_vault_the_file_does_not_list_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    set_on(&me, false).unwrap();
    unpair(&me, 9).unwrap();
    assert!(!dir.path().join(DEVICE_FILE).exists());
}

#[test]
fn an_app_cap_per_vault_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    for i in 1..=MAX_CLIENTS_PER_VAULT as i64 {
        pair(&me, pc(i, 'a')).unwrap();
    }
    assert!(pair(&me, pc(99, 'a')).is_err());
    assert_eq!(device_state(&me).clients.len(), MAX_CLIENTS_PER_VAULT);
}

#[test]
fn the_oldest_vault_falls_off_past_the_cap_and_is_then_simply_off() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..MAX_VAULTS + 2 {
        set_on(&here(dir.path(), &format!("vault-{i}")), true).unwrap();
    }
    assert!(!device_state(&here(dir.path(), "vault-0")).on);
    assert!(!device_state(&here(dir.path(), "vault-1")).on);
    assert!(device_state(&here(dir.path(), "vault-2")).on);
    assert!(device_state(&here(dir.path(), &format!("vault-{}", MAX_VAULTS + 1))).on);
    // Touching a vault again makes it the newest.
    set_on(&here(dir.path(), "vault-2"), true).unwrap();
    set_on(&here(dir.path(), "vault-new"), true).unwrap();
    assert!(device_state(&here(dir.path(), "vault-2")).on);
    assert!(!device_state(&here(dir.path(), "vault-3")).on);
}

#[test]
fn a_write_never_makes_the_file_too_big_to_read_back() {
    let dir = tempfile::tempdir().unwrap();
    // Sixteen apps each in twelve vaults is far past the file's size cap.
    for v in 0..12 {
        let me = here(dir.path(), &format!("vault-{v}"));
        set_on(&me, true).unwrap();
        for i in 1..=MAX_CLIENTS_PER_VAULT as i64 {
            pair(&me, pc(i, 'a')).unwrap();
        }
    }
    let bytes = std::fs::metadata(dir.path().join(DEVICE_FILE))
        .unwrap()
        .len();
    assert!(bytes <= MAX_FILE_BYTES, "{bytes} bytes");
    // The newest vault survived intact, the oldest fell off, and the file still reads.
    let newest = device_state(&here(dir.path(), "vault-11"));
    assert!(newest.on);
    assert_eq!(newest.clients.len(), MAX_CLIENTS_PER_VAULT);
    assert!(!device_state(&here(dir.path(), "vault-0")).on);
}

#[test]
fn a_copied_vault_in_another_folder_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    set_on(&me, true).unwrap();
    pair(&me, pc(1, 'a')).unwrap();
    let copy = me.moved_to("some-other-folder-tag");
    assert_eq!(device_state(&copy), DeviceState::default());
    // Switching on there replaces the entry: what the first folder held is not the copy's.
    set_on(&copy, true).unwrap();
    assert!(device_state(&copy).on);
    assert!(device_state(&copy).clients.is_empty());
    assert_eq!(device_state(&me), DeviceState::default());
}

#[test]
fn all_off_forgets_the_vault_wherever_it_was_and_only_it() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (here(dir.path(), "vault-one"), here(dir.path(), "vault-two"));
    for v in [&one, &two] {
        set_on(v, true).unwrap();
        pair(v, pc(1, 'a')).unwrap();
    }
    all_off(dir.path(), "vault-one").unwrap();
    assert_eq!(device_state(&one), DeviceState::default());
    assert!(device_state(&two).on);
    assert!(!text(dir.path()).contains("vault-one"));
    // Forgetting a vault the file doesn't list writes nothing, and no file at all is fine.
    let before = text(dir.path());
    all_off(dir.path(), "vault-one").unwrap();
    assert_eq!(text(dir.path()), before);
    let empty = tempfile::tempdir().unwrap();
    all_off(empty.path(), "vault-one").unwrap();
    assert!(!empty.path().join(DEVICE_FILE).exists());
}

#[test]
fn what_is_replayed_after_all_off_cannot_switch_anything_on() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "vault-one");
    set_on(&me, true).unwrap();
    pair(&me, pc(1, 'a')).unwrap();
    all_off(dir.path(), "vault-one").unwrap();
    // The entry is gone, so there is no `on` to flip back: switching OFF writes nothing, and a
    // pair written back (an old token's) lands in an entry that is off.
    set_on(&me, false).unwrap();
    assert_eq!(device_state(&me), DeviceState::default());
    pair(&me, pc(1, 'a')).unwrap();
    assert!(!device_state(&me).on);
}

#[test]
fn a_file_that_cannot_be_read_fails_all_off_so_a_restore_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be: it exists but cannot be read as a file.
    std::fs::create_dir(dir.path().join(DEVICE_FILE)).unwrap();
    assert!(all_off(dir.path(), "vault-one").is_err());
}
