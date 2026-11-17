# Changelog

The Map's own history. Its version is the one in `manifest.json`, separate from PM's (spec D12): a
change that touches only this folder bumps this version and adds an entry here.

## 1.0.0

- The Map as a mod: the documents laid out on one canvas, by project (each project a hub with its
  documents around it) or by meaning (similar documents close together), with a legend, pan and zoom,
  a card on hover, file names on the dots, and a click to read the document in PM's reader. It wears
  PM's theme, text size and colour-blind palette.
- Per-vault settings (arrangement, cohesion, file names, the most documents placed by meaning), kept
  with the vault and its backups.
- Different from the Map that was built into PM: the layout by meaning is worked out in the page (a
  principal-component projection, in a Worker) and saved, so the first time after a change shows a bar
  while it works; the downloadable enhanced (t-SNE) layout is gone; a hover card never shows a file's
  path; and the Map uses the system font. Documents added or removed while it is open are offered as a
  Refresh, not redrawn under your pointer.
- `publisher.commit` stays all zeros in the source tree (a manifest cannot hold the hash of the commit
  that contains it). `pack-mod --commit <sha>` stamps the real one into the package.
