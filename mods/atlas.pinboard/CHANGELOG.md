# Changelog

The Pinboard's own history. Its version is the one in `manifest.json`, separate from PM's (spec D12): a
change that touches only this folder bumps this version and adds an entry here.

## 1.0.0

- The Pinboard, PM's planning board as a mod: notes, task lists, timelines and folders on one free
  board you arrange by dragging, with the folder games, notes you can save as documents, and your
  dated timeline entries on the Calendar. It wears PM's theme, text size and colour-blind palette.
- Credits the person who made it: `authors` names Bobby Yu, shown after the publisher ("Made by Atlas
  · author: Bobby Yu"). `authors` is for credit only; the publisher (Atlas) is still what pins updates.
- Your board and its three preferences are kept in the mod's own storage (and in your backups), not in
  PM's settings. "Ask before deleting" moved from Settings into the Pinboard's header.
- A link in a note opens in your browser only when it is an https address to a website; PM refuses the
  rest (a mail link, an http link, an address that is a number or a name with no dot), and says so.
- A project name typed into a timeline that PM does not know asks you first ("Create project ...?")
  instead of making the project on its own.
- `publisher.commit` stays all zeros in the source tree (a manifest cannot hold the hash of the commit
  that contains it). `pack-mod --commit <sha>` stamps the real one into the package.
