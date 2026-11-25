# Changelog

The Pinboard's own history. Its version is the one in `manifest.json`, separate from PM's (spec D12): a
change that touches only this folder bumps this version and adds an entry here.

## 1.0.1

- The manifest now says `"answersSuspend": true`: the Pinboard saves what you were typing when PM
  is about to close it (on a tab switch, a pause, an update, an uninstall, a vault switch, turning on
  the app lock, or a quit) and
  tells PM when that is done. PM 3.188 waits for that answer only from a mod that says it gives one,
  so without this line PM would close the board without waiting for your last edit. Nothing else
  changes. 1.0.0 was never published; 1.0.1 is the first version people install.

## 1.0.0

- The Pinboard, PM's planning board as a mod: notes, task lists, timelines and folders on one free
  board you arrange by dragging, with the folder games, notes you can save as documents, and your
  dated timeline entries on the Calendar. It wears PM's theme, text size and colour-blind palette.
- Credits the person who made it: `authors` names Bobby Yu, shown after the publisher ("Made by Atlas
  · author: Bobby Yu"). `authors` is for credit only; the publisher (Atlas) is still what pins updates.
- Your board and its three preferences are kept in the mod's own storage (and in your backups), not in
  PM's settings. "Ask before deleting" moved from Settings into the Pinboard's header.
- A link in a note opens in your browser only when it is an https address to a website. A mail
  link, an http link, an address made of numbers, `localhost` and a name with no dot are drawn as
  plain text you can still select and copy, and a click does nothing.
- Your board is looked after: opening it never rewrites it, and only what you change is saved (a
  card that did not fit this window's width is drawn moved but stays where you put it in what is
  stored). A board saved by a version of the Pinboard this one does not know is shown as unreadable
  and left exactly as it is. Cards and fields this version cannot read are kept when you save.
- A board has a size limit of 1 MB for one save, and the header shows how full it is. Past it the
  Pinboard says "This board is too big to save" and keeps your change on screen until you make room.
- The Markdown view of a note (the editor you get when the rich one cannot keep a note exactly) now
  looks right, and it keeps your cursor and selection.
- If something else changed the board while the Pinboard was open (a project was renamed, say), the
  Pinboard reads it again and adds your changes to it instead of saving over it. What is not saved
  yet is saved when PM closes the tab.
- A drag that leaves the window still ends where you let go. The dates on your Calendar are sent
  every time the Pinboard opens, and again whenever a saved change touches a dated entry.
- A project name typed into a timeline that PM does not know makes PM ask you first before it
  creates a new project, instead of making the project on its own.
- `publisher.commit` stays all zeros in the source tree (a manifest cannot hold the hash of the commit
  that contains it). `pack-mod --commit <sha>` stamps the real one into the package.
