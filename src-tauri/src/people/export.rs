// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export (spec 5.3): people as a vCard 3.0 file and, if asked, their timeline as a spreadsheet.
//!
//! Every person goes through [`Card::from_person`] and is written from the card, so what this
//! writes is exactly what `vcard::parse_vcf` reads back (`export_then_import_is_identity`).
//!
//! **Nothing a person typed can change the file's structure.** Values are escaped (`\\`, `\;`, `\,`,
//! line breaks as `\n`), and a `TYPE=` parameter comes only from a fixed table: any other label is
//! written as the escaped value of a sibling `itemN.X-ABLabel` property, so a label such as
//! `x;TYPE=EVIL:y` stays a label. Lines are folded at 75 octets on UTF-8 boundaries, CRLF ends.
//! In the spreadsheet every text cell is quoted and a cell that starts with `=`, `+`, `-`, `@`, a
//! tab or a carriage return gets a leading apostrophe, because names and event titles come from
//! other people's invites and files.
//!
//! People in Trash are never exported. The save dialog is opened by the command (rule L-5: the
//! destination of decrypted data never comes from the webview); this module only builds the bytes
//! and writes them where the command says.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::card::{Card, CardDetail};
use super::vcard::network_of_label;
use super::{get_person_core, refuse};
use crate::error::Result;

/// Who an export covers.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum ExportScope {
    /// Everyone outside Trash.
    All,
    /// Everyone outside Trash who carries the label.
    Label(i64),
    /// These people (those in Trash are left out).
    Ids(Vec<i64>),
}

/// What an export built, ready to write.
pub struct Exported {
    pub people: usize,
    pub vcf: Vec<u8>,
    pub csv: Option<Vec<u8>>,
}

/// What an export wrote.
#[derive(Clone, Serialize)]
pub struct ExportDone {
    pub people: usize,
    /// The files written, vCard first.
    pub files: Vec<String>,
}

impl std::fmt::Debug for ExportDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportDone")
            .field("people", &self.people)
            .field("files", &self.files.len())
            .finish()
    }
}

/// The name the timeline spreadsheet is written under, beside the vCard file.
pub const TIMELINE_FILE: &str = "people-timeline.csv";
const TIMELINE_STEM: &str = "people-timeline";

// --- escaping and folding ------------------------------------------------------------------------------------

/// vCard TEXT escaping. A line break is `\n`; any other control character is dropped, so a value
/// can never start a new line of the file.
pub(crate) fn esc(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(unified.len() + 8);
    for c in unified.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Add one content line, folded at 75 octets on a character boundary, CRLF-terminated.
fn push_folded(out: &mut String, line: &str) {
    let mut width = 0;
    for c in line.chars() {
        let w = c.len_utf8();
        if width + w > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += w;
    }
    out.push_str("\r\n");
}

/// The `TYPE` a label may be written as, from a fixed table, per kind. Anything else is a custom
/// label and travels as an escaped `X-ABLabel` value.
fn type_for(kind: &str, label: &str) -> Option<&'static str> {
    match (kind, label) {
        ("email" | "phone" | "address", "home") => Some("HOME"),
        ("email" | "phone" | "address", "work") => Some("WORK"),
        ("phone", "mobile") => Some("CELL"),
        ("phone", "fax") => Some("FAX"),
        ("phone", "pager") => Some("PAGER"),
        ("phone", "car") => Some("CAR"),
        ("address", "postal") => Some("POSTAL"),
        _ => None,
    }
}

struct Writer {
    out: String,
    group: u32,
}

impl Writer {
    fn line(&mut self, group: Option<u32>, name: &str, params: &str, value: &str) {
        let line = match group {
            Some(g) => format!("item{g}.{name}{params}:{value}"),
            None => format!("{name}{params}:{value}"),
        };
        push_folded(&mut self.out, &line);
    }

    fn next_group(&mut self) -> u32 {
        self.group += 1;
        self.group
    }

    /// A property whose label is a `TYPE` when the table has it, else an `itemN.X-ABLabel`.
    /// `base` is the parameters that always go (`;TYPE=INTERNET`).
    fn labelled(&mut self, name: &str, kind: &str, label: &str, base: &str, value: &str) {
        if label.is_empty() {
            self.line(None, name, base, value);
        } else if let Some(t) = type_for(kind, label) {
            let params = if base.is_empty() {
                format!(";TYPE={t}")
            } else {
                format!("{base},{t}")
            };
            self.line(None, name, &params, value);
        } else {
            let g = self.next_group();
            self.line(Some(g), name, base, value);
            self.line(Some(g), "X-ABLabel", "", &esc(label));
        }
    }
}

fn part<'a>(d: &'a CardDetail, key: &str) -> &'a str {
    d.parts
        .as_ref()
        .and_then(|p| p.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The PM-only parameters of a job or school: its dates and whether it is current. A date is
/// written only if it is made of digits and dashes, so a parameter can never carry structure.
fn date_params(d: &CardDetail) -> String {
    let mut out = String::new();
    for (name, date) in [("X-PM-FROM", &d.from_date), ("X-PM-TO", &d.to_date)] {
        if let Some(date) = date
            .as_deref()
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '-'))
        {
            out.push_str(&format!(";{name}={date}"));
        }
    }
    if !d.is_current {
        out.push_str(";X-PM-CURRENT=0");
    }
    out
}

/// Write one card as a vCard 3.0 block.
pub fn write_card(out: &mut String, card: &Card) {
    let mut w = Writer {
        out: std::mem::take(out),
        group: 0,
    };
    w.line(None, "BEGIN", "", "VCARD");
    w.line(None, "VERSION", "", "3.0");
    if let Some(ident) = &card.ident {
        w.line(None, "UID", "", &esc(ident));
    }
    w.line(None, "FN", "", &esc(&card.display_name));
    let n = [
        &card.family_name,
        &card.given_name,
        &card.middle_name,
        &card.name_prefix,
        &card.name_suffix,
    ]
    .iter()
    .map(|s| esc(s))
    .collect::<Vec<_>>()
    .join(";");
    w.line(None, "N", "", &n);
    let nicknames: Vec<String> = card
        .details
        .iter()
        .filter(|d| d.kind == "nickname")
        .map(|d| esc(&d.value))
        .collect();
    if !nicknames.is_empty() {
        w.line(None, "NICKNAME", "", &nicknames.join(","));
    }
    if let Some(born) = &card.born_on {
        w.line(None, "BDAY", "", born);
    }
    let many_jobs = card.details.iter().filter(|d| d.kind == "work").count() > 1;
    for d in &card.details {
        let value = esc(&d.value);
        match d.kind.as_str() {
            "email" => w.labelled("EMAIL", "email", &d.label, ";TYPE=INTERNET", &value),
            "phone" => w.labelled("TEL", "phone", &d.label, "", &value),
            "address" => {
                let adr = if d.parts.is_some() {
                    [
                        "po_box",
                        "extended",
                        "street",
                        "city",
                        "region",
                        "postal_code",
                        "country",
                    ]
                    .iter()
                    .map(|k| esc(part(d, k)))
                    .collect::<Vec<_>>()
                    .join(";")
                } else {
                    format!(";;{value};;;;")
                };
                w.labelled("ADR", "address", &d.label, "", &adr);
            }
            "web" => match network_of_label(&d.label) {
                Some(n) => w.line(None, "X-SOCIALPROFILE", &format!(";TYPE={}", n.key), &value),
                None if d.label.is_empty() || d.label == "Website" => {
                    w.line(None, "URL", "", &value);
                }
                None => {
                    let g = w.next_group();
                    w.line(Some(g), "URL", "", &value);
                    w.line(Some(g), "X-ABLabel", "", &esc(&d.label));
                }
            },
            "date" => {
                if d.label == "Anniversary" {
                    w.line(None, "ANNIVERSARY", "", &value);
                } else if d.label.is_empty() {
                    w.line(None, "X-ABDATE", "", &value);
                } else {
                    let g = w.next_group();
                    w.line(Some(g), "X-ABDATE", "", &value);
                    w.line(Some(g), "X-ABLabel", "", &esc(&d.label));
                }
            }
            "relation" => {
                if d.label.is_empty() {
                    w.line(None, "X-ABRELATEDNAMES", "", &value);
                } else {
                    let g = w.next_group();
                    w.line(Some(g), "X-ABRELATEDNAMES", "", &value);
                    w.line(Some(g), "X-ABLabel", "", &esc(&d.label));
                }
            }
            "work" => {
                // With more than one job the company and title are tied by a group, so a reader
                // can pair them again.
                let g = many_jobs.then(|| w.next_group());
                let dept = part(d, "department");
                let org = if dept.is_empty() {
                    value
                } else {
                    format!("{value};{}", esc(dept))
                };
                // The dates ride on the ORG line as PM-only parameters, so reading PM's own file
                // back matches the job (a job's identity includes its start) and a past job
                // stays past.
                w.line(g, "ORG", &date_params(d), &org);
                let title = part(d, "title");
                if !title.is_empty() {
                    w.line(g, "TITLE", "", &esc(title));
                }
            }
            "location" => {
                let params = match d.from_date.as_deref() {
                    Some(from) if !from.is_empty() => format!(";X-PM-FROM={from}"),
                    _ => String::new(),
                };
                w.line(None, "X-PM-LOCATION", &params, &value);
            }
            // vCard has no property for these three, so they travel as PM-only ones (E12: the
            // file holds everything PM does, and reads back into PM whole).
            "school" => {
                let field = part(d, "field");
                let school = if field.is_empty() {
                    value
                } else {
                    format!("{value};{}", esc(field))
                };
                w.line(None, "X-PM-SCHOOL", &date_params(d), &school);
            }
            "former_name" => w.line(None, "X-PM-FORMER-NAME", "", &value),
            "custom" => {
                if d.label.is_empty() {
                    w.line(None, "X-PM-CUSTOM", "", &value);
                } else {
                    let g = w.next_group();
                    w.line(Some(g), "X-PM-CUSTOM", "", &value);
                    w.line(Some(g), "X-ABLabel", "", &esc(&d.label));
                }
            }
            // A nickname went above.
            _ => {}
        }
    }
    if !card.notes.is_empty() {
        w.line(None, "NOTE", "", &esc(&card.notes));
    }
    if !card.labels.is_empty() {
        let items: Vec<String> = card.labels.iter().map(|l| esc(l)).collect();
        w.line(None, "CATEGORIES", "", &items.join(","));
    }
    if !card.met_where.is_empty() {
        w.line(None, "X-PM-MET-WHERE", "", &esc(&card.met_where));
    }
    if let Some(on) = &card.met_on {
        w.line(None, "X-PM-MET-ON", "", on);
    }
    if let Some(day) = card.leap_day {
        w.line(None, "X-PM-LEAP-DAY", "", day.word());
    }
    w.line(None, "END", "", "VCARD");
    *out = w.out;
}

// --- the timeline spreadsheet -------------------------------------------------------------------------------------

/// One row of the timeline: one entry for one person.
pub struct TimelineRow {
    pub date: String,
    pub kind: String,
    pub ident: String,
    pub name: String,
    pub note: String,
    pub event_title: String,
}

/// One quoted cell. A leading `=`, `+`, `-`, `@`, tab or carriage return gets an apostrophe in
/// front, so a spreadsheet never reads the cell as a formula.
pub(crate) fn csv_cell(text: &str) -> String {
    let mut cell = String::with_capacity(text.len() + 3);
    if matches!(
        text.chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    ) {
        cell.push('\'');
    }
    cell.push_str(text);
    format!("\"{}\"", cell.replace('"', "\"\""))
}

/// The timeline as RFC 4180: every cell quoted, CRLF between rows.
pub fn timeline_csv(rows: &[TimelineRow]) -> String {
    let mut out = String::new();
    let header = [
        "date",
        "kind",
        "person_uid",
        "person_name",
        "note",
        "event_title",
    ];
    out.push_str(&header.map(csv_cell).join(","));
    out.push_str("\r\n");
    for r in rows {
        let cells = [&r.date, &r.kind, &r.ident, &r.name, &r.note, &r.event_title];
        out.push_str(&cells.map(|c| csv_cell(c)).join(","));
        out.push_str("\r\n");
    }
    out
}

// --- reading what to export ----------------------------------------------------------------------------------------

/// The ids an export covers, outside Trash, in list order.
fn scope_ids(conn: &Connection, scope: &ExportScope) -> Result<Vec<i64>> {
    let rows: Vec<i64> = match scope {
        ExportScope::All => {
            let mut stmt = conn
                .prepare("SELECT id FROM people WHERE trashed_at IS NULL ORDER BY sort_key, id")?;
            let rows = stmt
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        }
        ExportScope::Label(label_id) => {
            let mut stmt = conn.prepare(
                "SELECT p.id FROM people p JOIN person_labels pl ON pl.person_id = p.id \
                  WHERE pl.label_id = ?1 AND p.trashed_at IS NULL ORDER BY p.sort_key, p.id",
            )?;
            let rows = stmt
                .query_map(params![label_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        }
        ExportScope::Ids(ids) => {
            let json =
                serde_json::to_string(ids).map_err(|_| refuse("Those people can't be read."))?;
            let mut stmt = conn.prepare(
                "SELECT id FROM people WHERE trashed_at IS NULL \
                   AND id IN (SELECT value FROM json_each(?1)) ORDER BY sort_key, id",
            )?;
            let rows = stmt
                .query_map(params![json], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        }
    };
    Ok(rows)
}

/// The timeline entries of these people, oldest first, one row per person per entry. Interactions
/// have their own commands in a later slice; this reads the table directly.
fn timeline_rows(conn: &Connection, ids: &[i64]) -> Result<Vec<TimelineRow>> {
    let json = serde_json::to_string(ids).map_err(|_| refuse("Those people can't be read."))?;
    let mut stmt = conn.prepare(
        "SELECT i.on_date, i.kind, p.uid, p.display_name, i.note, COALESCE(i.event_title, '') \
           FROM interactions i \
           JOIN interaction_people ip ON ip.interaction_id = i.id \
           JOIN people p ON p.id = ip.person_id \
          WHERE p.id IN (SELECT value FROM json_each(?1)) \
          ORDER BY i.on_date, i.id, p.id",
    )?;
    let rows = stmt
        .query_map(params![json], |r| {
            Ok(TimelineRow {
                date: r.get(0)?,
                kind: r.get(1)?,
                ident: r.get(2)?,
                name: r.get(3)?,
                note: r.get(4)?,
                event_title: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Build the export of `scope`: the vCard bytes and, with `timeline`, the spreadsheet. Refuses an
/// export of nobody.
pub fn build(conn: &Connection, scope: &ExportScope, timeline: bool) -> Result<Exported> {
    let ids = scope_ids(conn, scope)?;
    if ids.is_empty() {
        return Err(refuse("There is nobody to export."));
    }
    let mut vcf = String::new();
    for &id in &ids {
        let person = get_person_core(conn, id)?;
        write_card(&mut vcf, &Card::from_person(&person));
    }
    let csv = if timeline {
        Some(timeline_csv(&timeline_rows(conn, &ids)?).into_bytes())
    } else {
        None
    };
    Ok(Exported {
        people: ids.len(),
        vcf: vcf.into_bytes(),
        csv,
    })
}

// --- writing --------------------------------------------------------------------------------------------------------

/// `stem.ext` in `dir`, or the first `stem-N.ext` that doesn't replace a file already there. It
/// refuses rather than hand back a name that exists.
fn free_name(dir: &Path, stem: &str, ext: &str) -> Result<PathBuf> {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(refuse(
        "That folder already holds too many files with this name. Pick another folder.",
    ))
}

/// Write the export where the user chose: the vCard at `dest` (a `.vcf` is added to a name with no
/// extension, and then to a free name, because the dialog only asked about `dest` itself), the
/// spreadsheet beside it. Each file is written whole or not at all.
pub fn write(dest: &Path, exported: &Exported) -> Result<ExportDone> {
    let failed = || refuse("PM couldn't write that file. Check the folder and try again.");
    let mut dest = dest.to_path_buf();
    if dest.extension().is_none() {
        let stem = dest
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
        let dir = dest.parent().map(Path::to_path_buf);
        match (dir, stem) {
            (Some(dir), Some(stem)) => dest = free_name(&dir, &stem, "vcf")?,
            _ => {
                dest.set_extension("vcf");
            }
        }
    }
    crate::vault::write_atomic(&dest, &exported.vcf).map_err(|_| failed())?;
    let mut files = vec![dest.to_string_lossy().into_owned()];
    if let Some(csv) = &exported.csv {
        let dir = dest.parent().unwrap_or_else(|| Path::new("."));
        let path = free_name(dir, TIMELINE_STEM, "csv")?;
        crate::vault::write_atomic(&path, csv).map_err(|_| failed())?;
        files.push(path.to_string_lossy().into_owned());
    }
    Ok(ExportDone {
        people: exported.people,
        files,
    })
}
