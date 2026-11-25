// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the MCP tests share: the API's test host with a vault OPEN in it (the core asks which vault
//! is open, as the app does), this device in that vault, and an app minted, paired and switched on.

use crate::mods::api::test_support::TestHost;
use crate::mods::devmode::Here;
use crate::mods::mcp::{clients, device, Bound};

/// Open a vault in the host's store (its folder is `<host dir>/vault-root`) and name this device in
/// it. The device's files live in the host's own folder, which is also the MCP server's `data_dir`.
pub(crate) fn open_vault(host: &TestHost) -> Here {
    use crate::mods::api::Host;
    let (runtime, vault_id, vault_root) = vault_runtime(host);
    host.store().set_vault_runtime(runtime).unwrap();
    Here::open(host.dir(), &vault_id, &vault_root).unwrap()
}

/// A runtime for the host's vault folder (the same vault every time it is asked), its id and folder.
pub(crate) fn vault_runtime(host: &TestHost) -> (crate::VaultRuntime, String, std::path::PathBuf) {
    let vault_root = host.dir().join("vault-root");
    std::fs::create_dir_all(&vault_root).unwrap();
    let meta = crate::vault::ensure_device_meta(&vault_root).unwrap();
    let resolved = crate::vault::resolve_layout(&vault_root, None);
    let master = crate::vault::master_from_db_key_hex(&"00".repeat(32)).unwrap();
    let runtime = crate::VaultRuntime::build(&resolved, &meta, &master);
    (runtime, meta.vault_id, vault_root)
}

/// An app that is minted, paired on this device and switched on.
pub(crate) struct App {
    pub id: i64,
    pub token: String,
    pub bound: Bound,
}

/// Mint an app holding `scopes` in the open vault and switch the server on for this device.
pub(crate) fn pair_app(host: &TestHost, here: &Here, scopes: &[&str]) -> App {
    use crate::mods::api::Host;
    let scopes: Vec<String> = scopes.iter().map(|s| (*s).to_owned()).collect();
    let (id, token) = host
        .sql(|conn| clients::mint(conn, here, "Desk", &scopes))
        .unwrap();
    device::set_on(here, true).unwrap();
    let store_epoch = host.store().store_epoch();
    App {
        id,
        token,
        bound: Bound {
            client: id,
            store_epoch,
        },
    }
}

/// The text of a tool result as JSON (a `tools/call` reply's first content item).
pub(crate) fn result_json(reply: &serde_json::Value) -> serde_json::Value {
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .expect("a tool result with text");
    serde_json::from_str(text).expect("the text is JSON")
}
