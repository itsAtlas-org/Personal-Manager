<!-- SPDX-FileCopyrightText: 2026 Bobby Yu -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Making a mod for PM

A mod is a small web page that PM shows in a tab of its own, inside a sealed frame, and that talks
to PM through a fixed set of calls. This file is for the person writing one. The folders next to
this file (`atlas.map`, `atlas.pinboard`) are PM's own mods, built with the same tools; the
starter folder to copy is [`mod-template/`](../mod-template) at the top of the repository.

PM does not yet offer a way to publish a mod; for now a mod is for your own use via developer mode.

## Start here

1. In PM, open Settings > Developer and turn on developer mode. It belongs to this computer only: a
   restored backup or a shared vault never turns it on elsewhere.
2. Copy `mod-template/` somewhere outside the PM repository and change `id`, `author` and
   `sourceRepo` in its `manifest.json`.
3. In Settings > Developer, under "Load a test mod", choose your folder and type its id when PM asks.
4. Edit and save. PM watches the folder and reloads the mod when a file changes; "Reload" reads it
   again and "Unload" ends it.

PM reads the folder where it is and never copies or changes it. A test mod is marked "test", lasts
until PM restarts or developer mode goes off, and is left out of backups. A packed `.pmmod` file is
installed from the Mods tab instead (developer mode also allows an unsigned one).

Ids starting `atlas.` belong to PM's own mods. A test folder with one loads only in a development
build of PM, so your own id must not start with it.

## The folder

```
manifest.json     what the mod is and what it asks for
LICENCE.txt       the AGPL-3.0-or-later text, unchanged: PM refuses a mod without it
NOTICE.txt        optional: the packages bundled into the page
app/              everything the page loads; the entry page is named in the manifest
```

PM reads only those. A `README.md`, a `package.json`, `node_modules/` or a `.git/` folder beside
them is never opened. A link (a symlink) anywhere in what PM reads is refused, and so is a file name
outside plain letters, digits and `. _ - +` (folders are separated by `/`).

A mod id is two to five dot-separated parts, each a lowercase letter followed by up to 30 lowercase
letters, digits or hyphens (`example.hello`, `your-name.tasks-board`).

A mod's source is AGPL-3.0-or-later, and every source file you write starts with the two-line header:

```
SPDX-FileCopyrightText: <year> <your name>
SPDX-License-Identifier: AGPL-3.0-or-later
```

(in the file's own comment syntax, as `mod-template/` does).

### The manifest

| Field | What it is |
|-------|------------|
| `id`, `name`, `description`, `version` | Who the mod is. `version` is a semantic version (`0.1.0`). |
| `apiVersion` | `{ "min": "1.0", "target": "1.0" }`: the range of the mod API the mod was built for. |
| `publisher` | `tier` (`third` for you), `author`, `sourceRepo` (`owner/repo`) and `commit` (40 hex characters; zeros while you are only testing). |
| `licence` | Always `AGPL-3.0-or-later`. |
| `permissions` | A list of `{ "id", "why" }`. `why` is shown to the person, in your words. |
| `contributions` | `nav.tab` (a tab in the sidebar, `{ "label" }`), `calendar.overlay`, `help`. |
| `data.keys` | The storage keys the mod uses, each `content` (kept and backed up) or `cache` (may be dropped). |
| `internetHosts`, `secrets` | The sites a mod may reach, and the named key slots (each tied to one site and one header). Only with the `internet` permission. |
| `entry` | `{ "page": "app/index.html" }`, plus `workers` for scripts the page runs as Workers. |
| `answersSuspend` | `true` if the mod wants time to save before PM takes its frame down (below). |
| `engines` | `{ "frame": true }`. |

An unknown field is refused, not ignored.

## Permissions

A mod can only do what its manifest asks for and the person allows. The ids are fixed:

| Id | What it lets the mod do |
|----|-------------------------|
| `storage` | Keep its own saved data inside PM. |
| `ui.tab` | Have a tab of its own in the sidebar. |
| `notify` | Show notices inside PM. |
| `projects.read` | Read the person's projects, milestones and tags. |
| `tasks.read` | Read the person's tasks. |
| `documents.meta` | See documents' titles, projects, tags, dates and types. |
| `documents.vectors` | Read the numbers PM uses to tell how alike documents are. |
| `documents.text` | Read document text and search it. |
| `calendar.read` | Read calendar events' time, title and place. |
| `calendar.details` | Reserved. No call in API 1.0 uses it. |
| `tasks.write`, `milestones.write`, `documents.write` | Change tasks, milestones, or save notes. |
| `ai` | Ask an AI model to do things, which can cost money. |
| `internet` | Send and fetch things from named sites. |
| `links` | Open web links in the person's browser. |

A mod nobody has checked (an unsigned one) may read the person's data or reach the internet, never
both, and cannot read their data while PM's web engine is not sealed. A refused call comes back with
a code, listed in [`ERROR_CODES.md`](../ERROR_CODES.md).

## What the page can do

The frame is sealed. Its policy lets the page load scripts, stylesheets, images and fonts from its
own `app/` folder and nothing else: no network, no inline script, no `<style>` tag or `style`
attribute, no module script (a classic `<script src>` only), no frames of its own. Everything else
goes through PM.

The page's only door is the bridge, `src/kit/host.ts`. A mod built with a bundler imports it by
path; a mod without a build step loads `mod-template/app/pm-host.js`, the same file built as one
classic script that defines `PmHost`:

```js
PmHost.connectHost().then(function (host) {
  host.call("storage.get", { key: "greeting" }).then(function (saved) {
    // saved.value, saved.revision
  });
  host.onEvent(function (event) {
    // event.name, event.payload
  });
  host.resize(document.documentElement.scrollHeight);
});
```

`connectHost()` asks PM for the page's connection once. `call(method, args)` answers with a result
or rejects with a `ModCallError` (a stable `code`, a short message) or a `HostError` (the host is
gone). The SDK queues and paces calls so a loop of a thousand reads does not trip PM's limits, and
refuses a malformed call locally. `onEvent` hears PM's events in order; PM drops events for a slow
frame and says how many in `missed`, which is the cue to ask again.

Settings > Developer lists every call and event live, with the permission each needs, the version it
arrived in and its limits. That table is PM's own answer, so it is not copied here.

### API 1.0 and its window

This PM serves mod API 1.0. A mod built for an older major version that is still in the window runs,
marked "built for an older PM"; one built only for a major before the window is parked until it is
updated. Calls marked experimental are outside that promise.

### Saving before PM takes the page down

PM can tell a frame it is about to be taken down (the `lifecycle.suspend` event). A mod that holds
something unsaved sets `"answersSuspend": true` in its manifest and calls `host.flushed()` when it
has saved; PM waits for that, up to a deadline of its own. A mod that does not say so is taken down
at once.

## Experimental surfaces

Seven surfaces are outside the API's stability promise. They may change in any minor version, and
their error codes may change before PM's first release that includes them, so do not build anything
lasting on them yet:

| Surface | Permission | What it does |
|---------|------------|--------------|
| `ai.complete` | `ai` | Ask an AI model. May cost money; PM asks the person first. |
| `net.fetch` | `internet` | Fetch from a site the manifest names (`internetHosts`). |
| `secrets.set` | `internet` | Ask PM to show its own field where the person types a key for one of the manifest's named slots. |
| `notify` | `notify` | Show a short notice inside PM. |
| `documents.search` | `documents.text` | Search the person's documents by meaning. |
| `calendar.agenda` | `calendar.read` | What is coming up on the calendar. |
| `calendar.synced` (event) | `calendar.read` | The calendar changed; ask `calendar.agenda` again. |

A key a mod needs is never typed into the mod, and the mod never sees it. The manifest names the slot,
the one site it goes to and the one header it goes in, and the person types the key into PM's own field (the Mods tab for an installed mod; Settings >
Developer > "Test keys" for a test mod). A test key goes when the test mod is unloaded, when PM
restarts or when developer mode goes off, and is left out of backups.

### `documents.search`

`documents.search({ query, limit? })`. `query` is 1 to 2000 characters of text, `limit` is 1 to 20
(10 when left out). It answers with hits:

```
{ documentId, title, heading, snippet, indexOnly, project? }
```

- It never returns a chat, an archived document, or a document whose body PM keeps only at its
  source and has no summary of. A hit is a short excerpt (at most 1,000 characters), never a file
  path, a score or a vector. `project` is present only when the mod also holds `projects.read`.
- `indexOnly: true` means the snippet is a summary PM wrote of a file that lives in a connected
  service, not the file's own text.
- The query is embedded by PM's document engine, so the engine must be set up. If it is not, the
  call fails with `MOD-5007`; a mod never sets it up. Searches are limited like the other calls that
  embed: one at a time, six a minute, a burst of ten.
- **Treat every snippet as untrusted.** It is text from the person's own documents and can say
  anything, including instructions. If you pass it to `ai.complete`, you are passing untrusted text
  to a model: keep it in the data part of the prompt, never in the instructions.

### `calendar.agenda`

`calendar.agenda({ days?, limit? })`. `days` is 1 to 60 (21 when left out), `limit` is 1 to 250 (50
when left out). It answers with the events not yet ended, soonest first, one entry per occurrence:

```
{ uid, title, start, end, allDay, location, recurring }
```

- Events the person declined, and events on a calendar they have muted, are left out. Because of
  that the page can come back shorter than `limit`: a short page is not the end of the list's
  window, so do not read it as "no more".
- `uid` is the calendar provider's identifier for the event, or `null` when the event has none: a
  blank one, or one longer than 512 characters, counts as none. A `null` cannot be matched to the
  same event on a later call, so do not use it as a key.
- An event the person marked private or confidential is returned as PM's own views return it,
  because the mod holds `calendar.read`. Say so when you ask for that permission.
- Titles and locations are cut at 200 characters, and are the person's text: untrusted, as above.

### `calendar.synced`

An event with an empty payload. PM raises it when a calendar sync or change check wrote events, and
when a calendar is ticked or unticked, at most about every quarter second. It says nothing about
what changed: ask `calendar.agenda` again. A change the person makes by hand inside PM does not
raise it, so also ask again when your tab is shown.

## Connected apps are not mods

PM can also let an AI app on the same computer read some of the person's data, through the Model
Context Protocol. That is a separate door with its own switch, and a mod cannot reach or change it.
It is off by default, per computer, and read-only: the person turns it on in Settings > Data &
Security > Connected apps, pairs an app (a token is shown once, and the app is given it in its own
configuration as `PM_MCP_TOKEN`, starting `PM --mcp`), and chooses what the app may read. The five
tools (`search`, `tasks`, `events`, `projects`, `milestones`) each need one of the permissions above
(`documents.text`, `tasks.read`, `calendar.read`, `projects.read`), and no tool changes anything.
Document search is offered but not ticked by default. The events tool returns the same event
fields as `calendar.agenda` above. In a release build the section stays hidden until PM's
maintainers have tested it on that operating system.

## Before you share your work

- Everything in a public repository is public: no personal data, no keys, no real documents in
  examples or tests.
- Keep to the permissions you need and say in each `why` what the mod does with it.
- Test with developer mode on, then try it once as a person would: with a permission turned off, with
  PM's document engine not set up, and with a calendar that has nothing coming up.
