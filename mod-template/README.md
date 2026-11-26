<!-- SPDX-FileCopyrightText: 2026 Bobby Yu -->
<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Starter mod

A small, working mod: one tab that remembers a greeting. Copy this folder, change the id and the
author in `manifest.json`, and load it as a test mod. The whole guide is in
[`mods/README.md`](../mods/README.md).

```
manifest.json     what the mod is and what it asks for
LICENCE.txt       the licence text (AGPL-3.0-or-later); PM refuses a mod without it
app/index.html    the page: two classic scripts, no inline code
app/pm-host.js    PM's page-side SDK, generated: do not edit it by hand
app/main.js       your code
app/style.css     your styles
```

`app/pm-host.js` is built from PM's `src/kit/host.ts` and checked by a test, so it always matches
the PM it sits in. To pick up a newer one, copy the file from a newer copy of this folder.
