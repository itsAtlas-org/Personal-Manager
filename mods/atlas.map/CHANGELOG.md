# Changelog

The Document Map's own history. Its version is the one in `manifest.json`, separate from PM's (spec D12): a
change that touches only this folder bumps this version and adds an entry here.

## 1.1.0

- Named the Document Map, so it isn't mistaken for a geographical map: the mod, its tab, its page
  heading and its help say "Document Map". The id (`atlas.map`) stays.
- Credits the person who made it: the manifest names Bobby Yu in `authors`, shown after the
  publisher ("Made by Atlas · credited in the mod to Bobby Yu"). `authors` is for credit only; the
  publisher (Atlas) is still what pins updates.

## 1.0.0

- The Map as a mod: the documents laid out on one canvas, by project (each project a hub with its
  documents around it) or by meaning (similar documents close together), with a legend, pan and zoom,
  a card on hover, file names on the dots, and a click to read the document in PM's reader. It wears
  PM's theme, text size and colour-blind palette.
- Per-vault settings (arrangement, cohesion, file names, the most documents placed by meaning), kept
  with the vault and its backups.
- Different from the Map that was built into PM: the layout by meaning is worked out in the page (a
  principal-component projection, in a Worker) and saved, so the first time after a change shows a bar
  while it works; the downloadable enhanced (t-SNE) layout isn't in this version yet and is planned for the
  mod itself, so it is not dropped for good; a hover card never shows a file's
  path; and the Map uses the system font. Documents added or removed while it is open are offered as a
  Refresh, not redrawn under your pointer.
- `publisher.commit` stays all zeros in the source tree (a manifest cannot hold the hash of the commit
  that contains it). `pack-mod --commit <sha>` stamps the real one into the package.
