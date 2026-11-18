# Security Policy

PM is alpha software built by a tiny team. We take security seriously — it's a
local-first app that holds sensitive personal data — but we're small, so please
read this for what to expect: best-effort timelines, honest communication, and
no bug bounty.

## Threat model (the short version)

PM is **local-first**: your data lives on your own machine. Outbound traffic is limited and
enumerable — the model API calls that power chat, sent to your cloud AI provider by default or,
if you configure one, to a local model endpoint (a loopback server on this machine sends nothing
off-box; a remote or LAN endpoint receives the requests you route to it, and PM refuses to send a
token and chats in the clear to a public address); a launch-time update check against the
signed release feed (and the download if you accept an update); a fetch of PM's signed mod safety
list (two small files from the project's public `PM-Mods` GitHub release, at launch and every six
hours, sending nothing but a generic `PM-mods/1` user agent, accepted only with the project's
signature; plus, for a mod you install from the catalogue, its package and its updates from the same place, checked against that list's hash before PM reads them). PM also installs the Document Map once from that same catalogue, for a vault that already had documents when the Map stopped being built in, and says so on the screen where you turn it on; the websites each installed mod declares it will fetch from and you allow (a mod reaches only those exact hostnames, over HTTPS on the standard port, to the public internet only and never to an address on this machine or your network, with no proxy, and a fixed `PM-mod/1` user agent), and, if a mod uses AI, the requests it makes through your own Background AI route (see below); a one-time first-run download
of PM's on-device models and Python dependencies (your local model runner, if you use one, fetches
model weights itself — PM does not); and, only if you configure them, a calendar fetch
(read-only unless you turn on editing for a Google account, after which PM also sends the
event changes you make to Google), a read-only sync of the cloud accounts you connect (Google
Drive, Google Sheets, OneDrive), and encrypted backups to your own cloud. PM itself has no
telemetry, analytics, or crash reporting, and switches off the usage reporting of the libraries in
its document engine (onnxruntime, huggingface_hub, hf-xet) and the system profile pip would send
PyPI. Two components do reporting of their own: on Windows, Microsoft's Edge WebView2 Runtime
sends Microsoft its required diagnostic data (no app can turn that off) and optional data when
Windows allows it, while PM starts it with custom crash reporting on
(`IsCustomCrashReportingEnabled`), so its crash dumps stay on the computer, in WebView2's own crash
folder. Three runs are exceptions, where crash reports follow Windows' settings: one where PM had to
fall back to WebView2's default environment, and one on a WebView2 Runtime too old to have the
option (both leave its webview engine unsealed), and possibly the first launch after the update
that brings this, if the previous version's WebView2 browser process was still running, which may
not show as unsealed; and the Proton Drive CLI reports to Proton if the Proton account's
"Collect usage diagnostics" was on when the CLI last signed in (it reads the setting only then), and
can send a crash report if it fails while starting up, before it has read the setting. Beyond
WebView2's crash dumps, PM can't change either. **Calendar editing raises the stakes of a stolen token:** a Google
account with editing on holds a token that can create, change and delete events on every
calendar that account can edit, put them on another of its calendars, add video calls, and send
invitations, and replies to invitations, in its name.
"Turn off editing" in PM stops
PM using it at once but doesn't narrow what Google granted. Disconnecting the account removes
the grant at Google only when nothing else in PM uses that account; while Google Drive or
backups still do, the calendar write permission stays granted until you remove PM at
myaccount.google.com/permissions. At rest, the **SQLCipher store** (settings, search index) is encrypted with
a key held in the OS keychain. The webview's **`localStorage` holds interface state only** — theme
and accent, panel sizes, view modes, which sections you have folded away — while anything that names
your content (project names, calendar ids, the cloud accounts you back up to) lives in the encrypted
store, not beside it. **How PM's secrets (the database key, your API key, any OAuth
tokens) are stored differs by platform.** On **macOS** they share a *single keychain item*, so one
"Always Allow" covers all of them instead of a separate prompt per secret — a deliberate
convenience trade-off, and one worth stating plainly: that single grant is *coarser* than a
per-secret grant, so any code running as PM, once you allow it, can read every secret rather than
just the one it asked for. On **Windows and Linux** each secret is its own keychain item, because
Windows Credential Manager caps one credential at 2560 bytes and PM's secrets together exceed it;
neither platform prompts per item, so nothing is lost by keeping them separate. On Windows, a
secret PM saves for the first time (an API key added where none was saved, an account connected for
the first time, the database key of a new install) is kept in Credential Manager for **this computer only** (`CRED_PERSIST_LOCAL_MACHINE`), so
it doesn't follow you to other PCs with a work or school roaming profile. A secret saved before
keeps the setting it had, and PM keeps that setting when it updates it, so no other computer that
shares it loses it and nothing needs reconnecting. Two limits follow: a secret that already roams
keeps roaming, even when PM replaces its value, until it is removed and saved again (by
disconnecting and reconnecting an account, or, for the OpenRouter keys and the database key, only
through Remove PM data), and on a roaming profile shared with a PC running an older PM, that PC may
still save roaming copies. **A mod's saved keys are the one exception to "the OS keychain".** A key you type for a mod (a
password or API key for a site the mod declares) is kept in PM's encrypted database, not in the
keychain, because the keychain holds the database key itself and Windows caps how much one item can
hold. Anyone who can open the vault can read it, which includes another account that shares
the vault. A key is bound to the one site and header the mod declared when you saved it and is only ever
added by PM to a request to that site; the mod never sees the value. Backups and exports leave mod keys
out, so after a restore you type them again. Wherever PM's secrets are
stored, the secrets themselves never leave the Rust backend. Your **documents live in a plaintext Markdown vault**,
so their at-rest protection relies on your own OS full-disk encryption (BitLocker /
FileVault) — a deliberate choice to keep your notes openable by any tool, not a gap
to report. If you share a vault with another account on the same PC, **changing its passphrase never
transfers ownership without an explicit confirmation** — a re-key carries the recorded owner
forward, and a takeover is a separate, confirmed act that PM records in the vault's
tamper-evident metadata. PM treats **all ingested content as untrusted data,
never as instructions** — documents, calendar feeds, and model output can't make
PM act on your behalf. The areas where a bug would matter most are: the
**encrypted local store** (SQLCipher), **secret handling** (the OS keychain
holds your API key, the database key, and any OAuth tokens), and the **signed
release / auto-updater pipeline** (a bad update is the highest-leverage way to
reach a user's machine). Reports that touch those areas are the most valuable.

**A mod's AI use and its money limit.** A mod that asks for `ai` uses your own Background AI route
(Settings → AI & Models), every model in that list including a local one, and PM asks for a
monthly limit in US dollars before it first can cost money. Cloud calls ask for zero data retention
and a local model stays on your own endpoint. The limit and the spend are kept in PM's database
with the mod's record, so two edges are accepted: uninstalling and reinstalling a mod starts its month's spend at
zero again, and restoring an older backup puts the spend back to what it was then.

## Supported versions

PM auto-updates, and only the **most recent release** receives security fixes.
Older alpha builds are unsupported — if you're on one, update to the latest
release. There are no long-term support branches.

| Version              | Supported          |
| -------------------- | ------------------ |
| Latest release       | :white_check_mark: |
| Any earlier build    | :x:                |

(Versioning note: the earliest pre-release builds were tagged `1.x`, sorting
below the first public `2.0.0-alpha`; alpha releases have continued from there.
Whatever the number, only the newest release is supported.)

## Reporting a vulnerability

**Please do not open a public issue for security bugs.** Public issues are
visible to everyone before there's a fix.

Report privately through GitHub's private vulnerability reporting:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability**.
3. Fill in the form — this opens a private advisory only the maintainers can see.

<!-- TODO(Bobby): if you want a fallback contact email for people who can't use
     GitHub's private reporting, add it here. Left blank deliberately rather
     than guessing one. -->

Please include:

- **Steps to reproduce** (a proof of concept if you have one).
- The **affected version** (see Settings → What's New, or the title bar).
- The **impact** as you understand it — what an attacker could do.
- Any **diagnostic code** PM showed (e.g. `SBX-2104`) and, for a sidecar-sandbox
  concern, the **Developer mode → Sidecar sandbox** readout. See
  [`ERROR_CODES.md`](ERROR_CODES.md) for what the codes mean.

What to expect:

- A **best-effort acknowledgement within a few days**. We're a small team, so
  this isn't a guaranteed SLA.
- **Status updates** as we investigate and work on a fix.
- **Coordinated disclosure** — please give us reasonable time to ship a fix
  before disclosing publicly. We'll credit you if you'd like once it's resolved.

## Scope

**In scope:**

- The **PM desktop app** (Tauri/Rust backend + the web frontend).
- The **Python sidecar** (document conversion, embeddings, on-device photo OCR).
- The **build / release / signing pipeline** (CI, the signed update feed, the updater).

**Out of scope:**

- **Third-party providers** — OpenRouter and the AI models themselves. Report
  those to the relevant vendor.
- Issues that **require an already-compromised host or OS** (e.g. malware or an
  attacker who already has your account, your keychain, or filesystem access).
- **Social engineering**, phishing, or physical access.

Thanks for helping keep PM and its users safe.
