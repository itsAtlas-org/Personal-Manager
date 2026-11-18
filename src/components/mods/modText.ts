// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a mod supplied (its name, its maker, its reasons) as one plain line. React already draws it as
// text, so markup and links are not the danger; the danger is invisible characters. A right-to-left
// override in a name reorders what follows it, so a mod can be made to read as something it isn't, and
// a line break lets a "reason" pose as a separate PM sentence. Control, format (which includes the
// bidirectional controls and zero-width characters) and line/paragraph separators become spaces, runs
// of spaces collapse, and the ends are trimmed. A run of combining marks is cut short too (a name of
// one letter and thirty-nine accents would stack over the badges beside it). The rule is
// lib/modTextClean; the sidebar cleans a mod's tab name the same way (lib/nav.ts).
//
// `quoteName` is for a name inside one of PM's own sentences or titles: quoted, so it can't pose as
// the words around it.

export {
  hasAbout,
  madeByLine,
  makerCredit,
  modBody,
  plainLine as modText,
  quoteName,
} from "../../lib/modTextClean";
