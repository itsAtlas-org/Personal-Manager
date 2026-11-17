// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The detail card's date, the way PM writes every date (design system V2): DD-MM-YYYY, dropping the
// year in the current year. Ported from src/lib/format.ts `formatDate` (a mod cannot import it: it
// reaches only src/kit). An ISO timestamp is read as the instant it names and shown in the viewer's own
// calendar; a value that is not a date is shown as it is, never as "NaN".

const pad2 = (n: number) => String(n).padStart(2, "0");

export function formatDate(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const day = pad2(d.getDate());
  const month = pad2(d.getMonth() + 1);
  const year = d.getFullYear();
  return year === now.getFullYear() ? `${day}-${month}` : `${day}-${month}-${year}`;
}
