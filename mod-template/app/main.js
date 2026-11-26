// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The starter mod's page script. It is a classic script (not a module) and runs inside PM's sealed
// frame: it can reach PM only through the host, never the network or PM's own page.
//
// What it shows, in the order it does it:
//   1. connect to PM (`connectHost`), once;
//   2. read and write one saved value (`storage.get` / `storage.set`);
//   3. wear PM's theme (`theme.get`, and `theme.changed` as an event);
//   4. tell PM how tall the page is (`resize`).
// Everything it asks for is covered by the two permissions in manifest.json: `ui.tab` and `storage`.
// A call a mod has no permission for is refused with a code (see ERROR_CODES.md), never half done.

/* global PmHost */

(function () {
  "use strict";

  var KEY = "greeting"; // declared under "data.keys" in manifest.json
  var status = document.getElementById("status");
  var input = document.getElementById("greeting");
  var save = document.getElementById("save");

  // The revision of the saved value we last saw: `storage.set` must say which one it is based on, so
  // two writers (two windows of this mod) cannot silently overwrite each other.
  var revision = 0;

  // PM's theme arrives as a set of CSS custom properties. Setting them on the root with setProperty
  // is allowed by the frame's policy (a style attribute or a style tag is not).
  function wear(theme) {
    if (!theme || typeof theme.vars !== "object" || theme.vars === null) return;
    Object.keys(theme.vars).forEach(function (name) {
      if (/^--[a-z0-9][a-z0-9-]*$/.test(name) && typeof theme.vars[name] === "string") {
        document.documentElement.style.setProperty(name, theme.vars[name]);
      }
    });
  }

  // Tell PM how tall the page wants to be, so its tab fits the content.
  function fit(host) {
    host.resize(document.documentElement.scrollHeight);
  }

  function start(host) {
    status.textContent = "Connected.";

    // Events arrive in order. Subscribe before awaiting anything else so none is missed.
    host.onEvent(function (event) {
      if (event.name === "theme.changed") wear(event.payload);
    });

    host
      .call("theme.get")
      .then(wear)
      .catch(function () {
        // The page still works in the browser's own colours.
      });

    host
      .call("storage.get", { key: KEY })
      .then(function (saved) {
        revision = saved.revision;
        input.value = typeof saved.value === "string" ? saved.value : "";
        input.disabled = false;
        save.disabled = false;
        fit(host);
      })
      .catch(function (error) {
        status.textContent = "Could not read what was saved: " + error.message;
      });

    save.addEventListener("click", function () {
      save.disabled = true;
      host
        .call("storage.set", { key: KEY, value: input.value, expectedRevision: revision })
        .then(function (done) {
          revision = done.revision;
          status.textContent = "Saved.";
        })
        .catch(function (error) {
          // MOD-4001 means another window saved first (the error's `revision` is the current one); a
          // real mod would read again and retry. This one just says so.
          status.textContent = "Could not save: " + error.message;
        })
        .then(function () {
          save.disabled = false;
        });
    });
  }

  PmHost.connectHost().then(start, function (error) {
    // Opened outside PM (a double-clicked file), or PM never answered.
    status.textContent = "This page runs inside PM: " + error.message;
  });
})();
