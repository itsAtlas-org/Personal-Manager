# PM diagnostic codes

When part of PM fails in a way worth pinpointing, it shows a short **diagnostic code** like
`SBX-2104`. These codes are **stable** — quoting one in a bug report maps it to the exact spot in
the code, which turns "it didn't work" into something we can find and fix quickly.

**When you see a code, include it** — verbatim, with the message next to it and your operating
system — in your report. For a normal bug, open a GitHub issue; for a **security vulnerability**,
follow [`SECURITY.md`](SECURITY.md) instead of opening a public issue.

## `SBX-####` — sidecar worker sandbox

PM reads and converts your files in a background **worker** process. On supported systems that
worker runs inside an OS **sandbox**: no network access, and read access to only a handful of
folders — so even a malicious file can't use PM's file reader to reach the internet or your vault.
If the sandbox can't be set up, PM keeps working with the worker **unconfined** (this is a
defence-in-depth layer on top of the already-offline worker and at-rest encryption, not a gate) and
reports *why* with one of these codes. You can see the live state any time in **Developer mode →
Sidecar sandbox**, and every fall-back is written to the app log as `running unconfined — [SBX-…]`.

Ranges: `1xxx` cross-platform · `2xxx` Windows · `3xxx` macOS · `4xxx` Linux.

| Code | What it means |
|------|---------------|
| `SBX-1101` | Couldn't locate the worker's runtime folder, so the sandbox couldn't be anchored. |
| `SBX-1102` | Couldn't resolve the on-device model-cache folder. |
| `SBX-1103` | Couldn't read the base Python interpreter's location from the virtual environment. |
| `SBX-1104` | Couldn't create the staging folder the worker reads its inputs from. |
| `SBX-1105` | Couldn't copy an input file into the staging folder (that one request runs unsandboxed). |
| `SBX-1106` | The sandboxed worker process failed to launch. |
| `SBX-2101` | Windows: couldn't create the AppContainer profile or derive its identity. |
| `SBX-2102` | Windows: couldn't grant the worker access to the staging folder. |
| `SBX-2103` | Windows: couldn't grant the worker access to the sidecar-script folder. |
| `SBX-2104` | Windows: couldn't grant the worker access to the model-cache folder. |
| `SBX-2105` | Windows: couldn't grant the worker access to the Python / virtual-environment folders. |
| `SBX-3101` | macOS: `sandbox-exec` isn't present, so the worker couldn't be confined (it runs unconfined). |
| `SBX-3106` | macOS: the sandboxed worker couldn't load its libraries or read its model cache under the profile, so it was re-run unconfined. |
| `SBX-4101` | Linux: couldn't build the Landlock filesystem ruleset on a kernel that supports it. |
| `SBX-4102` | Linux: no network-blocking filter is available for this CPU architecture. |
| `SBX-4105` | Linux: **degraded, not failed** — Landlock isn't available (kernel older than 5.13, or its security module is off), so the worker's network is still blocked but its filesystem is *not* restricted. |
| `SBX-4106` | Linux: the sandboxed worker couldn't load its libraries under the sandbox, so it was re-run unconfined. |

> On Linux, "confined" means both **Landlock** (filesystem) and **seccomp** (network) are active;
> `SBX-4105` is the honest middle state where only the network half applies. On macOS, "confined"
> means `sandbox-exec` applied the profile (network denied and the filesystem restricted together);
> it has no degraded middle state — the profile applies whole, or the worker runs unconfined.

## `MOD-####` — mod calls

A mod asks PM for things (your documents' titles, its own saved settings, PM's theme) through PM's
mod API. When PM refuses or can't complete one of those calls, the mod receives one of these codes
with a short message, and a well-made mod tells you what happened. A mod may also show the code
itself: quote it, with the mod's name, when you report a problem with a mod.

Ranges: `1xxx` the call itself · `2xxx` permission · `3xxx` limits · `4xxx` the data · `5xxx` the
state of PM or the mod · `6xxx` the network · `9xxx` PM's own failure.

The AI, network and key calls (`ai.complete`, `net.fetch`, `secrets.set`, `notify`) are experimental:
they are outside the mod API's stability promise, and their codes below, `MOD-3004`, `MOD-3005`,
`MOD-5004` to `MOD-5006` and the `6xxx` range, may still change before PM's first release that
includes them. A message never carries what a website or an AI provider said, or the address asked
for: only PM's own sentence.

| Code | What it means |
|------|---------------|
| `MOD-1001` | There is no call by that name in this version of PM. |
| `MOD-1002` | The call was removed from PM's mod API; the mod needs an update. |
| `MOD-1003` | The call's arguments aren't what it takes (a missing or unknown field, a malformed key). |
| `MOD-2001` | The mod doesn't have the permission the call needs: it never asked for it, you turned it off, it wasn't granted at install, or a safety rule refuses it (unsigned mods may read your data or reach the internet, never both; and they can't read your data while PM's web engine isn't sealed). |
| `MOD-2002` | The call needs a permission PM asks you for the first time it's used, and you haven't been asked yet. |
| `MOD-3001` | The mod is calling PM too fast, or has too many calls running at once (for an AI request or a web request, the limits are tighter than for other calls). |
| `MOD-3002` | The call's arguments, or its answer, are larger than PM allows. |
| `MOD-3003` | The mod's private storage is full. |
| `MOD-3004` | The mod has used up its monthly AI limit (the amount you let it spend on AI models, set in the Mods tab), or it has no limit set yet for a model that costs money. |
| `MOD-3005` | The call took longer than PM allows and was stopped. Whatever it was waiting for (an AI model, a website) didn't answer in time. |
| `MOD-4001` | The mod tried to save over a value that changed since it read it (another of its windows saved first); it should read it again. |
| `MOD-4002` | Nothing was found, or nothing the mod is allowed to see (PM never tells the two apart). |
| `MOD-5001` | PM is locked, or its data is closed (another profile has the vault open, or it's still locked). |
| `MOD-5002` | The mod isn't running: it's paused, parked, still installing, or not turned on yet. |
| `MOD-5003` | The mod's session ended before the call finished (its tab closed, it was updated or uninstalled, PM locked, or the vault changed). |
| `MOD-5004` | PM has no AI model set up to answer with, so an AI request can't run. Choose a model in Settings. |
| `MOD-5005` | PM can't tell what the AI model costs, so it can't be sure the request is within the mod's limit, and doesn't run it. |
| `MOD-5006` | The AI model didn't answer (it was unreachable, refused, or failed). PM doesn't pass on why. |
| `MOD-6001` | The web address isn't one the mod may use: it isn't on the list of websites the mod declared, or it isn't a plain secure (https) address. |
| `MOD-6002` | The website's address isn't on the public internet (it points at this computer or a private network), so PM won't connect. |
| `MOD-6003` | PM couldn't find the website: its name didn't resolve. |
| `MOD-6004` | PM couldn't reach the website in time (the computer is offline, the website refused, or it was too slow). |
| `MOD-6005` | The website sent PM to another address too many times. |
| `MOD-6006` | The website's answer is larger than PM allows a mod to receive. |
| `MOD-6007` | The mod needs a key for that website, and you haven't entered it (or it was cleared when the mod updated). Enter it in the Mods tab. |
| `MOD-9001` | PM couldn't complete the call. The details are in PM's log; the mod only gets this code. |

---

*Maintainers: the authoritative lists are the `sbx` module in
[`src-tauri/src/sidecar.rs`](src-tauri/src/sidecar.rs) and `Code` in
[`src-tauri/src/mods/api/error.rs`](src-tauri/src/mods/api/error.rs) (a test there fails when this
`MOD` table and `Code` disagree). Keep the tables in sync when you add a code, and **never renumber
or reuse a shipped one** — they travel in logs and bug reports, and mods branch on the `MOD` codes.*
