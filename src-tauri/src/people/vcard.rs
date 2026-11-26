// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vCard parser (spec 5.1): pure, hand-rolled, no crate, no regex, no recursion, no I/O.
//!
//! [`parse_vcf`] takes the bytes of a `.vcf` file and returns its people as [`Card`]s, plus what it
//! skipped and what it had to say about the file. It is liberal on purpose: vCard 2.1 from Android
//! and Outlook (bare `TEL;CELL;VOICE:` parameters, quoted-printable values with soft line breaks,
//! `CHARSET=`), 3.0 from Apple and Google (`item1.EMAIL` property groups with a sibling `X-ABLabel`,
//! a year-less birthday written as year 1604) and 4.0 (`--0415`, `KIND:org`, `SOCIALPROFILE`) all
//! parse, and a bad card never stops the file. The pipeline, in order:
//!
//! 1. **Bytes in**, never `String::from_utf8` on the whole file: a BOM is sniffed (UTF-8, UTF-16),
//!    and a value that isn't valid UTF-8 decodes as Windows-1252 and counts a warning.
//! 2. **Lines** split on CRLF, LF or a lone CR and unfolded on bytes (a fold can land mid-character),
//!    one logical line at a time, cut at [`LINE_MAX`] while unfolding. A `PHOTO`, `LOGO`, `SOUND` or
//!    `KEY` is stepped over without ever building its line, so a photo-heavy export costs about the
//!    file's size and one card.
//! 3. **Quoted-printable soft breaks** (a value ending in `=` continues on the next line) glue that
//!    line *without* stripping its leading whitespace (the Thunderbird bug class), inside the same
//!    cap.
//! 4. **Tokenising** `[group.]name *(;param):value` with a quote-aware scan; names, parameter names
//!    and `BEGIN:VCARD`/`END:VCARD` are case-insensitive; a bare 2.1 parameter is `TYPE=`.
//! 5. **Structured values** (`N`, `ADR`, `ORG`, `NICKNAME`, `CATEGORIES`) are split on an unescaped
//!    `;` or `,` *before* anything is unescaped.
//! 6. **Mapping** (notes 06 section D): the properties PM keeps become the fields of a [`Card`];
//!    an Apple `_$!<Work>!$_` label loses its wrapper and an item-grouped `X-ABLabel` overrides TYPE.
//! 7. **Dates** accept `YYYY-MM-DD`, `YYYYMMDD`, `--MM-DD`, `--MMDD` and date-times (the date as
//!    written, no zone shift); year 1604, year 0000 or the year named by `X-APPLE-OMIT-YEAR` mean
//!    "no year".
//! 8. **Ignored and counted by property name only**, never by value.
//!
//! **Nothing here ever fetches anything**: a `PHOTO;VALUE=uri`, `SOURCE`, `FBURL` and the rest are
//! ignored, so a hostile file can't make PM connect anywhere (a test scans this module's imports).
//! URLs are kept as inert text. No contact value reaches a log or a warning: the warnings and the
//! ignored list hold property names and counts only.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Map, Value};

use super::card::{truncate_bytes, truncate_chars, Card, CardDetail};
use super::partial_date::PartialDate;

/// The biggest file read. Photo-heavy exports are big; the message that refuses a bigger one says
/// that an export without photos works.
pub const FILE_CAP: usize = 128 * 1024 * 1024;
/// Cards one file may hold.
pub const CARDS_MAX: usize = 25_000;
/// Properties one card keeps.
pub const PROPS_PER_CARD_MAX: usize = 500;
/// Properties kept across a whole file; past it the parse stops and `complete` is false.
pub const PROPS_TOTAL_MAX: usize = 1_000_000;
/// Bytes of one logical line, cut while unfolding.
pub const LINE_MAX: usize = 1024 * 1024;
/// Bytes of a `NOTE`.
pub const NOTE_MAX: usize = 64 * 1024;
/// Bytes of any other value (a decoded part of it).
pub const VALUE_MAX: usize = 4 * 1024;
/// How many different ignored property names are listed before the rest are lumped together.
const IGNORED_NAMES_MAX: usize = 200;
/// Longest property name listed.
const NAME_MAX: usize = 64;
/// Parameters read from one property.
const PARAMS_MAX: usize = 64;
/// Bytes of one parameter value (an address `LABEL=` is the long one).
const PARAM_VALUE_MAX: usize = 2 * 1024;

/// Every limit, so a test can exercise a clamp without a 25,000-card fixture (the `ics.rs`
/// pattern).
#[derive(Clone, Copy)]
pub struct Caps {
    pub cards: usize,
    pub props_per_card: usize,
    pub props_total: usize,
    pub line: usize,
    pub note: usize,
    pub value: usize,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            cards: CARDS_MAX,
            props_per_card: PROPS_PER_CARD_MAX,
            props_total: PROPS_TOTAL_MAX,
            line: LINE_MAX,
            note: NOTE_MAX,
            value: VALUE_MAX,
        }
    }
}

/// What the parser has to say about a file, by kind. Counts of cards or values, never a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    /// A card with no name, nickname, company, email or phone: skipped.
    NoName,
    /// A value that wasn't valid in its character set and was read as Windows-1252.
    BadEncoding,
    /// A value or line cut to the cap.
    ValueCut,
    /// A date that wasn't a date (kept as text where it could be).
    BadDate,
    /// A card that never reached its `END:VCARD` before the next `BEGIN:VCARD`.
    UnfinishedCard,
    /// A card with more properties than PM keeps.
    TooManyProperties,
    /// A card with more details or labels than a person holds.
    TooManyDetails,
    /// A line inside a card that is not `name:value`.
    UnreadableLine,
}

impl Warning {
    /// The word the wire carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Warning::NoName => "no_name",
            Warning::BadEncoding => "bad_encoding",
            Warning::ValueCut => "value_cut",
            Warning::BadDate => "bad_date",
            Warning::UnfinishedCard => "unfinished_card",
            Warning::TooManyProperties => "too_many_properties",
            Warning::TooManyDetails => "too_many_details",
            Warning::UnreadableLine => "unreadable_line",
        }
    }
}

/// What a parse returns. Holds contact values (the cards): no `Debug` derive.
pub struct VcfParse {
    pub cards: Vec<Card>,
    /// False when the file ended inside a card, or a cap stopped the parse early.
    pub complete: bool,
    pub warnings: BTreeMap<Warning, usize>,
    /// Properties set aside, by name (`PHOTO`, `X-ABUID`, ...) and how many of each.
    pub ignored: BTreeMap<String, usize>,
}

impl std::fmt::Debug for VcfParse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VcfParse")
            .field("cards", &self.cards.len())
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// Parse the bytes of a `.vcf` file with the production limits.
pub fn parse_vcf(bytes: &[u8]) -> VcfParse {
    parse_vcf_capped(bytes, Caps::default())
}

/// [`parse_vcf`] with the limits injected, for tests.
pub fn parse_vcf_capped(bytes: &[u8], caps: Caps) -> VcfParse {
    let mut warnings: BTreeMap<Warning, usize> = BTreeMap::new();
    let converted;
    let data: &[u8] = match sniff(bytes) {
        Encoding::Utf8Bom(rest) | Encoding::Plain(rest) => rest,
        Encoding::Utf16(rest, little) => {
            converted = utf16_to_utf8(rest, little, &mut warnings);
            &converted
        }
    };
    let mut parser = Parser {
        caps,
        cards: Vec::new(),
        card: None,
        warnings,
        ignored: BTreeMap::new(),
        props_total: 0,
        complete: true,
        stop: false,
    };
    let mut lines = Lines { data, pos: 0 };
    while let Some((first, next)) = lines.peek() {
        if parser.stop {
            break;
        }
        lines.pos = next;
        parser.logical_line(first, &mut lines);
    }
    if parser.card.is_some() {
        // The file ends inside a card: half a card is never imported.
        parser.card = None;
        parser.complete = false;
    }
    VcfParse {
        cards: parser.cards,
        complete: parser.complete,
        warnings: parser.warnings,
        ignored: parser.ignored,
    }
}

// --- 1. bytes in -------------------------------------------------------------------------------------

enum Encoding<'a> {
    Utf8Bom(&'a [u8]),
    Utf16(&'a [u8], bool),
    Plain(&'a [u8]),
}

/// A BOM names the encoding; with none, a file that starts `B\0E\0` (or `\0B\0E`) is UTF-16 that
/// left it out.
fn sniff(bytes: &[u8]) -> Encoding<'_> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Encoding::Utf8Bom(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Encoding::Utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Encoding::Utf16(rest, false);
    }
    match bytes {
        [b'B' | b'b', 0, b'E' | b'e', 0, ..] => Encoding::Utf16(bytes, true),
        [0, b'B' | b'b', 0, b'E' | b'e', ..] => Encoding::Utf16(bytes, false),
        _ => Encoding::Plain(bytes),
    }
}

fn utf16_to_utf8(bytes: &[u8], little: bool, warnings: &mut BTreeMap<Warning, usize>) -> Vec<u8> {
    let (pairs, _) = bytes.as_chunks::<2>();
    let units = pairs.iter().map(|&c| {
        if little {
            u16::from_le_bytes(c)
        } else {
            u16::from_be_bytes(c)
        }
    });
    let mut out = String::with_capacity(bytes.len() / 2);
    for unit in char::decode_utf16(units) {
        match unit {
            Ok(c) => out.push(c),
            Err(_) => {
                out.push('\u{fffd}');
                *warnings.entry(Warning::BadEncoding).or_insert(0) += 1;
            }
        }
    }
    out.into_bytes()
}

// --- 2. lines ------------------------------------------------------------------------------------------

/// The physical lines of the file, split on CRLF, LF or a lone CR.
struct Lines<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lines<'a> {
    /// The next line and where the one after it starts, without consuming it.
    fn peek(&self) -> Option<(&'a [u8], usize)> {
        if self.pos >= self.data.len() {
            return None;
        }
        let rest = &self.data[self.pos..];
        let end = rest
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .unwrap_or(rest.len());
        let mut next = self.pos + end;
        if next < self.data.len() {
            if self.data[next] == b'\r' && self.data.get(next + 1) == Some(&b'\n') {
                next += 2;
            } else {
                next += 1;
            }
        }
        Some((&rest[..end], next))
    }
}

fn starts_with_space(line: &[u8]) -> bool {
    matches!(line.first(), Some(b' ' | b'\t'))
}

/// Add a piece of a line, up to the cap, cutting on a character boundary. Once cut, nothing more
/// is added (the rest of the logical line is still read past).
fn push_piece(buf: &mut Vec<u8>, piece: &[u8], limit: usize, cut: &mut bool) {
    if *cut {
        return;
    }
    let room = limit.saturating_sub(buf.len());
    if piece.len() <= room {
        buf.extend_from_slice(piece);
        return;
    }
    let mut at = room;
    while at > 0 && piece.get(at).is_some_and(|b| b & 0xC0 == 0x80) {
        at -= 1;
    }
    buf.extend_from_slice(&piece[..at]);
    *cut = true;
}

/// Where a content line's head (`group.NAME;params`) ends, found incrementally so a line that is
/// folded many times is scanned once, and whether the head names quoted-printable.
#[derive(Default)]
struct HeadScan {
    scanned: usize,
    in_quote: bool,
    colon: Option<usize>,
    qp: bool,
}

impl HeadScan {
    fn update(&mut self, buf: &[u8]) {
        if self.colon.is_some() {
            return;
        }
        let from = self.scanned.min(buf.len());
        for (i, &b) in buf.iter().enumerate().skip(from) {
            if b == b'"' {
                self.in_quote = !self.in_quote;
            } else if b == b':' && !self.in_quote {
                self.colon = Some(i);
                self.qp = buf[..i]
                    .windows(16)
                    .any(|w| w.eq_ignore_ascii_case(b"QUOTED-PRINTABLE"));
                break;
            }
        }
        self.scanned = buf.len();
    }
}

/// The group, the name and where the name ends, of a content line. The group is the token before a
/// first `.` that comes before any `;` or `:` (so a value containing `item1.` is never a group).
/// A content line's group, name and the index where the name ends.
type NameSplit<'a> = (Option<&'a [u8]>, &'a [u8], usize);

fn split_name(line: &[u8]) -> Option<NameSplit<'_>> {
    let first = line.iter().position(|&b| matches!(b, b'.' | b';' | b':'))?;
    let mut group = None;
    let mut name_start = 0;
    if line[first] == b'.' {
        let g = &line[..first];
        if !g.is_empty() && g.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-') {
            group = Some(g);
            name_start = first + 1;
        }
    }
    let end = name_start
        + line[name_start..]
            .iter()
            .position(|&b| matches!(b, b';' | b':'))?;
    let name = &line[name_start..end];
    if name.is_empty()
        || name.len() > 128
        || !name
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return None;
    }
    Some((group, name, end))
}

/// The properties whose bytes are never kept: images, sounds and keys.
fn skipped_name(line: &[u8]) -> Option<&'static str> {
    let (_, name, _) = split_name(line)?;
    ["PHOTO", "LOGO", "SOUND", "KEY"]
        .into_iter()
        .find(|s| name.eq_ignore_ascii_case(s.as_bytes()))
}

// --- 4. tokenising -------------------------------------------------------------------------------------------

struct Param {
    /// Upper-cased.
    name: String,
    values: Vec<String>,
}

struct Head {
    /// Upper-cased.
    group: Option<String>,
    /// Upper-cased.
    name: String,
    params: Vec<Param>,
    /// Where the value starts in the line.
    value_at: usize,
}

fn upper(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_ascii_uppercase()
}

/// RFC 6868: `^n` is a line break, `^^` a caret, `^'` a double quote.
fn param_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek() {
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('^') => {
                    out.push('^');
                    chars.next();
                }
                Some('\'') => {
                    out.push('"');
                    chars.next();
                }
                _ => out.push('^'),
            }
        } else {
            out.push(c);
        }
    }
    truncate_bytes(&out, PARAM_VALUE_MAX).0
}

/// Tokenise `[group.]name *(;param):value`. `None` when the line has no unquoted colon or no valid
/// name.
fn parse_head(line: &[u8]) -> Option<Head> {
    let (group, name, name_end) = split_name(line)?;
    let n = line.len();
    let mut pos = name_end;
    let mut params: Vec<Param> = Vec::new();
    loop {
        match line.get(pos)? {
            b':' => {
                return Some(Head {
                    group: group.map(upper),
                    name: upper(name),
                    params,
                    value_at: pos + 1,
                });
            }
            b';' => {
                pos += 1;
                let start = pos;
                while pos < n && !matches!(line[pos], b'=' | b';' | b':') {
                    pos += 1;
                }
                let token = &line[start..pos];
                if line.get(pos) == Some(&b'=') {
                    pos += 1;
                    let mut values = Vec::new();
                    loop {
                        let mut raw: Vec<u8> = Vec::new();
                        if line.get(pos) == Some(&b'"') {
                            pos += 1;
                            while pos < n && line[pos] != b'"' {
                                raw.push(line[pos]);
                                pos += 1;
                            }
                            // The closing quote, then anything stray up to the next delimiter.
                            pos = (pos + 1).min(n);
                            while pos < n && !matches!(line[pos], b',' | b';' | b':') {
                                pos += 1;
                            }
                        } else {
                            while pos < n && !matches!(line[pos], b',' | b';' | b':') {
                                raw.push(line[pos]);
                                pos += 1;
                            }
                        }
                        values.push(param_text(&raw));
                        if line.get(pos) == Some(&b',') {
                            pos += 1;
                        } else {
                            break;
                        }
                    }
                    if params.len() < PARAMS_MAX {
                        params.push(Param {
                            name: upper(token),
                            values,
                        });
                    }
                } else if !token.is_empty() && params.len() < PARAMS_MAX {
                    // A bare 2.1 parameter: `TEL;HOME;VOICE:` is `TYPE=HOME,VOICE`.
                    params.push(Param {
                        name: "TYPE".to_string(),
                        values: vec![param_text(token)],
                    });
                }
            }
            _ => return None,
        }
    }
}

fn param<'a>(params: &'a [Param], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.values.first())
        .map(String::as_str)
}

/// Every TYPE token, lower-cased, whether given as `TYPE=a,b`, `TYPE=a;TYPE=b` or bare.
fn type_tokens(params: &[Param]) -> Vec<String> {
    params
        .iter()
        .filter(|p| p.name == "TYPE")
        .flat_map(|p| p.values.iter())
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .collect()
}

// --- 5. decoding values -----------------------------------------------------------------------------------------

/// How a property's bytes become text.
struct ValueCtx {
    qp: bool,
    charset: Option<String>,
}

impl ValueCtx {
    fn of(params: &[Param]) -> ValueCtx {
        let qp = params.iter().any(|p| {
            (p.name == "ENCODING" || p.name == "TYPE")
                && p.values
                    .iter()
                    .any(|v| v.trim().eq_ignore_ascii_case("QUOTED-PRINTABLE"))
        });
        ValueCtx {
            qp,
            charset: param(params, "CHARSET").map(|c| c.trim().to_ascii_lowercase()),
        }
    }
}

fn warn(warnings: &mut BTreeMap<Warning, usize>, w: Warning) {
    *warnings.entry(w).or_insert(0) += 1;
}

/// Split on an unescaped `sep` (a backslash escapes the byte after it), keeping at most `max`
/// parts. Done on the raw bytes, before any unescaping or quoted-printable decoding.
fn split_unescaped(raw: &[u8], sep: u8, max: usize) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\\' => i += 1,
            b if b == sep => {
                parts.push(&raw[start..i]);
                start = i + 1;
                if parts.len() >= max {
                    return parts;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if parts.len() < max {
        parts.push(&raw[start.min(raw.len())..]);
    }
    parts
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Quoted-printable to bytes. An `=` not followed by two hex digits stays as it is.
fn qp_decode(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'=' {
            let high = raw.get(i + 1).copied().and_then(hex_val);
            let low = raw.get(i + 2).copied().and_then(hex_val);
            if let (Some(h), Some(l)) = (high, low) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// Windows-1252 for the bytes 0x80 to 0x9F (every other byte is its own code point).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

fn cp1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if (0x80..0xA0).contains(&b) {
                char::from_u32(u32::from(CP1252_HIGH[usize::from(b - 0x80)])).unwrap_or('\u{fffd}')
            } else {
                char::from(b)
            }
        })
        .collect()
}

/// Bytes to text in the declared character set. Latin-1 and Windows-1252 are read as Windows-1252;
/// anything else is read as UTF-8, and bytes that aren't valid UTF-8 fall back to Windows-1252 with
/// a warning. (Legacy East Asian sets would need a crate; a file in one reads as mojibake, never
/// fails.)
fn decode_charset(
    bytes: &[u8],
    charset: Option<&str>,
    warnings: &mut BTreeMap<Warning, usize>,
) -> String {
    let latin = matches!(
        charset,
        Some(
            "iso-8859-1"
                | "iso8859-1"
                | "latin1"
                | "latin-1"
                | "windows-1252"
                | "cp1252"
                | "us-ascii"
                | "ascii"
                | "iso-8859-15"
        )
    );
    if latin {
        return cp1252(bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            warn(warnings, Warning::BadEncoding);
            cp1252(bytes)
        }
    }
}

/// Undo vCard TEXT escaping: `\n` and `\N` are a line break, `\x` is `x`, a lone trailing
/// backslash is dropped.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One value part: quoted-printable, then the character set, then the escapes, cut to `limit`.
fn decode_part(
    raw: &[u8],
    ctx: &ValueCtx,
    limit: usize,
    warnings: &mut BTreeMap<Warning, usize>,
) -> String {
    let bytes: Cow<[u8]> = if ctx.qp {
        Cow::Owned(qp_decode(raw))
    } else {
        Cow::Borrowed(raw)
    };
    let text = unescape(&decode_charset(&bytes, ctx.charset.as_deref(), warnings));
    let (text, was_cut) = truncate_bytes(&text, limit);
    if was_cut {
        warn(warnings, Warning::ValueCut);
    }
    text
}

/// A name part that may hold a comma list (`Smith,Jones`): the items joined with a space.
fn decode_joined(
    raw: &[u8],
    ctx: &ValueCtx,
    limit: usize,
    warnings: &mut BTreeMap<Warning, usize>,
) -> String {
    split_unescaped(raw, b',', 16)
        .into_iter()
        .map(|p| decode_part(p, ctx, limit, warnings))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// --- the parser ----------------------------------------------------------------------------------------------------

/// A property kept for the card being read, already decoded and split.
struct Prop {
    group: Option<String>,
    name: String,
    params: Vec<Param>,
    parts: Vec<String>,
}

impl Prop {
    fn first(&self) -> &str {
        self.parts.first().map(String::as_str).unwrap_or("")
    }

    fn is_pref(&self) -> bool {
        self.params.iter().any(|p| p.name == "PREF")
            || type_tokens(&self.params).iter().any(|t| t == "pref")
    }
}

#[derive(Default)]
struct CardState {
    props: Vec<Prop>,
    over: bool,
}

struct Parser {
    caps: Caps,
    cards: Vec<Card>,
    card: Option<CardState>,
    warnings: BTreeMap<Warning, usize>,
    ignored: BTreeMap<String, usize>,
    props_total: usize,
    complete: bool,
    stop: bool,
}

/// The properties the mapping reads; everything else is ignored and counted.
fn is_mapped(name: &str) -> bool {
    matches!(
        name,
        "FN" | "N"
            | "NICKNAME"
            | "EMAIL"
            | "TEL"
            | "ADR"
            | "URL"
            | "X-SOCIALPROFILE"
            | "SOCIALPROFILE"
            | "IMPP"
            | "X-SKYPE"
            | "X-AIM"
            | "X-GOOGLE-TALK"
            | "X-JABBER"
            | "X-MSN"
            | "X-ICQ"
            | "X-YAHOO"
            | "X-QQ"
            | "ORG"
            | "TITLE"
            | "ROLE"
            | "BDAY"
            | "ANNIVERSARY"
            | "X-ANNIVERSARY"
            | "X-ABDATE"
            | "X-ABRELATEDNAMES"
            | "RELATED"
            | "NOTE"
            | "CATEGORIES"
            | "UID"
            | "REV"
            | "KIND"
            | "X-ABLABEL"
            | "X-GOOGLE-LABEL"
            | "X-PM-MET-WHERE"
            | "X-PM-MET-ON"
            | "X-PM-LOCATION"
            | "X-PM-SCHOOL"
            | "X-PM-FORMER-NAME"
            | "X-PM-CUSTOM"
    )
}

impl Parser {
    fn warn(&mut self, w: Warning) {
        warn(&mut self.warnings, w);
    }

    fn ignore(&mut self, name: &str) {
        let (key, _) = truncate_chars(name, NAME_MAX);
        let key = if !self.ignored.contains_key(&key) && self.ignored.len() >= IGNORED_NAMES_MAX {
            "(others)".to_string()
        } else {
            key
        };
        *self.ignored.entry(key).or_insert(0) += 1;
    }

    /// Read one logical line starting at `first`, unfolding it, and handle it.
    fn logical_line(&mut self, first: &[u8], lines: &mut Lines<'_>) {
        if first.is_empty() {
            return;
        }
        if let Some(name) = skipped_name(first) {
            // Step over the property without building its line. A base64 body whose lines aren't
            // indented (some 2.1 exporters) has no colon in it, which tells it from the next
            // property.
            let b64 = first.windows(6).any(|w| w.eq_ignore_ascii_case(b"BASE64"));
            while let Some((next, pos)) = lines.peek() {
                if starts_with_space(next) || (b64 && !next.is_empty() && !next.contains(&b':')) {
                    lines.pos = pos;
                } else {
                    break;
                }
            }
            if self.card.is_some() {
                self.ignore(name);
            }
            return;
        }
        let limit = self.caps.line;
        let mut buf: Vec<u8> = Vec::with_capacity(first.len().min(limit) + 64);
        let mut cut = false;
        push_piece(&mut buf, first, limit, &mut cut);
        let mut tail_eq = first.last() == Some(&b'=');
        let mut head = HeadScan::default();
        loop {
            head.update(&buf);
            let Some((next, pos)) = lines.peek() else {
                break;
            };
            if head.qp && tail_eq {
                // A quoted-printable soft break: the next line continues the value, and its
                // leading whitespace is part of it.
                lines.pos = pos;
                if !cut && buf.last() == Some(&b'=') {
                    buf.pop();
                }
                push_piece(&mut buf, next, limit, &mut cut);
                tail_eq = next.last() == Some(&b'=');
            } else if starts_with_space(next) {
                lines.pos = pos;
                let piece = &next[1..];
                push_piece(&mut buf, piece, limit, &mut cut);
                tail_eq = piece.last() == Some(&b'=');
            } else {
                break;
            }
        }
        self.handle(&buf, cut);
    }

    fn handle(&mut self, buf: &[u8], cut: bool) {
        let Some(head) = parse_head(buf) else {
            if self.card.is_some() {
                self.warn(Warning::UnreadableLine);
            }
            return;
        };
        let raw = &buf[head.value_at..];
        match head.name.as_str() {
            "BEGIN" => {
                if is_vcard(raw) {
                    self.begin();
                }
                return;
            }
            "END" => {
                if is_vcard(raw) && self.card.is_some() {
                    self.finish_card();
                }
                return;
            }
            _ => {}
        }
        if self.card.is_none() {
            return;
        }
        if cut {
            self.warn(Warning::ValueCut);
        }
        self.property(head, raw);
    }

    fn begin(&mut self) {
        if self.card.is_some() {
            // The previous card never reached its END: the BEGIN proves its exporter moved on, so
            // what it held is kept, and the file says so.
            self.warn(Warning::UnfinishedCard);
            self.finish_card();
        }
        if self.cards.len() >= self.caps.cards {
            self.stop = true;
            self.complete = false;
            return;
        }
        self.card = Some(CardState::default());
    }

    fn property(&mut self, head: Head, raw: &[u8]) {
        if head.name == "VERSION" {
            return;
        }
        if !is_mapped(&head.name) {
            self.ignore(&head.name);
            return;
        }
        let Some(state) = self.card.as_ref() else {
            return;
        };
        if state.props.len() >= self.caps.props_per_card {
            if !state.over {
                self.warn(Warning::TooManyProperties);
                if let Some(s) = self.card.as_mut() {
                    s.over = true;
                }
            }
            return;
        }
        if self.props_total >= self.caps.props_total {
            self.stop = true;
            self.complete = false;
            return;
        }
        self.props_total += 1;
        let prop = self.decode(head, raw);
        if let Some(state) = self.card.as_mut() {
            state.props.push(prop);
        }
    }

    /// Split a value by its property's shape and decode each part.
    fn decode(&mut self, head: Head, raw: &[u8]) -> Prop {
        let ctx = ValueCtx::of(&head.params);
        let limit = if head.name == "NOTE" {
            self.caps.note
        } else {
            self.caps.value
        };
        let mut parts: Vec<String> = Vec::new();
        match head.name.as_str() {
            "N" => {
                for p in split_unescaped(raw, b';', 5) {
                    parts.push(decode_joined(p, &ctx, limit, &mut self.warnings));
                }
            }
            "ADR" => {
                for p in split_unescaped(raw, b';', 7) {
                    parts.push(decode_part(p, &ctx, limit, &mut self.warnings));
                }
            }
            "ORG" | "X-PM-SCHOOL" => {
                for p in split_unescaped(raw, b';', 8) {
                    parts.push(decode_part(p, &ctx, limit, &mut self.warnings));
                }
            }
            "NICKNAME" | "CATEGORIES" => {
                for p in split_unescaped(raw, b',', 64) {
                    parts.push(decode_part(p, &ctx, limit, &mut self.warnings));
                }
            }
            _ => parts.push(decode_part(raw, &ctx, limit, &mut self.warnings)),
        }
        Prop {
            group: head.group,
            name: head.name,
            params: head.params,
            parts,
        }
    }

    fn finish_card(&mut self) {
        let Some(state) = self.card.take() else {
            return;
        };
        match build_card(&state.props, &mut self.warnings) {
            Built::Card(card) => self.cards.push(*card),
            Built::NoName => self.warn(Warning::NoName),
            Built::NotAPerson => self.ignore("KIND"),
        }
    }
}

fn is_vcard(raw: &[u8]) -> bool {
    raw.trim_ascii().eq_ignore_ascii_case(b"VCARD")
}

// --- 6. mapping ----------------------------------------------------------------------------------------------------

enum Built {
    Card(Box<Card>),
    NoName,
    NotAPerson,
}

/// A network PM knows by its host: the label it shows, the `TYPE` an export writes, the hosts, and
/// the other names a file may give it.
pub(super) struct Network {
    pub label: &'static str,
    pub key: &'static str,
    hosts: &'static [&'static str],
    aliases: &'static [&'static str],
}

/// The short table of networks (spec 5.1): a URL on one of these hosts, or a social profile of one
/// of these types, gets the network as its label.
pub(super) const NETWORKS: [Network; 9] = [
    Network {
        label: "LinkedIn",
        key: "linkedin",
        hosts: &["linkedin.com"],
        aliases: &[],
    },
    Network {
        label: "Instagram",
        key: "instagram",
        hosts: &["instagram.com"],
        aliases: &[],
    },
    Network {
        label: "Facebook",
        key: "facebook",
        hosts: &["facebook.com", "fb.com"],
        aliases: &[],
    },
    Network {
        label: "X",
        key: "twitter",
        hosts: &["x.com", "twitter.com"],
        aliases: &["x"],
    },
    Network {
        label: "GitHub",
        key: "github",
        hosts: &["github.com"],
        aliases: &[],
    },
    Network {
        label: "Spotify",
        key: "spotify",
        hosts: &["spotify.com"],
        aliases: &[],
    },
    Network {
        label: "TikTok",
        key: "tiktok",
        hosts: &["tiktok.com"],
        aliases: &[],
    },
    Network {
        label: "YouTube",
        key: "youtube",
        hosts: &["youtube.com", "youtu.be"],
        aliases: &[],
    },
    Network {
        label: "Threads",
        key: "threads",
        hosts: &["threads.net", "threads.com"],
        aliases: &[],
    },
];

/// The network a label names exactly, for the export to write a social profile.
pub(super) fn network_of_label(label: &str) -> Option<&'static Network> {
    NETWORKS.iter().find(|n| n.label == label)
}

/// The host of a URL-ish value, lower-cased, without `www.`, user info or port.
fn host_of(value: &str) -> String {
    let rest = value.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = authority.split(':').next().unwrap_or(authority);
    let host = host.to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// The network a URL is on, if it is a known one.
pub(super) fn network_of_url(value: &str) -> Option<&'static Network> {
    let host = host_of(value);
    NETWORKS.iter().find(|n| {
        n.hosts
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    })
}

/// The label for a social-profile type ("linkedin" is "LinkedIn"; an unknown one has its first
/// letter raised).
fn social_label(kind: &str) -> String {
    let lower = kind.trim().to_ascii_lowercase();
    if let Some(n) = NETWORKS
        .iter()
        .find(|n| n.key == lower || n.aliases.contains(&lower.as_str()))
    {
        return n.label.to_string();
    }
    title_case(kind.trim())
}

fn title_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// An Apple label: `_$!<Work>!$_` loses its wrapper and the built-in ones read in PM's lower case;
/// anything else is the label as written.
fn apple_label(text: &str) -> String {
    let t = text.trim();
    let Some(inner) = t.strip_prefix("_$!<").and_then(|x| x.strip_suffix(">!$_")) else {
        return t.to_string();
    };
    match inner.to_ascii_lowercase().as_str() {
        "home" => "home".into(),
        "work" => "work".into(),
        "other" => "other".into(),
        "mobile" => "mobile".into(),
        "main" => "main".into(),
        "pager" => "pager".into(),
        "workfax" => "work fax".into(),
        "homefax" => "home fax".into(),
        "iphone" => "iPhone".into(),
        "homepage" => "Homepage".into(),
        _ => inner.to_string(),
    }
}

/// The label a TYPE says, for a kind: the tokens that mean something, `cell` read as `mobile`.
fn type_label(prop: &Prop, kind: &str) -> String {
    let skip: &[&str] = &[
        "internet",
        "pref",
        "voice",
        "x400",
        "text",
        "uri",
        "value",
        "quoted-printable",
        "base64",
        "b",
        "8bit",
        "7bit",
    ];
    let mut out: Vec<String> = Vec::new();
    for t in type_tokens(&prop.params) {
        if skip.contains(&t.as_str()) {
            continue;
        }
        if kind == "address" && matches!(t.as_str(), "dom" | "intl" | "parcel") {
            continue;
        }
        let t = if t == "cell" { "mobile".to_string() } else { t };
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out.join(" ")
}

/// The date a card value says, as PM stores it (`YYYY-MM-DD` or `--MM-DD`), or `None` when it isn't
/// a real date. Year 1604, year 0000 and the year `omit_year` names mean "no year".
fn parse_card_date(raw: &str, omit_year: Option<&str>) -> Option<String> {
    let text = raw.trim();
    let text = text.split(['T', 't']).next().unwrap_or(text).trim();
    let (month, day, year): (u8, u8, Option<u16>) = if let Some(rest) = text.strip_prefix("--") {
        let digits: String = rest.chars().filter(|c| *c != '-').collect();
        if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        (digits[..2].parse().ok()?, digits[2..].parse().ok()?, None)
    } else {
        let digits: String = text.chars().filter(|c| *c != '-').collect();
        if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let year: u16 = digits[..4].parse().ok()?;
        let omit = omit_year.and_then(|y| y.trim().parse::<u16>().ok());
        let year = if year == 0 || year == 1604 || omit == Some(year) {
            None
        } else {
            Some(year)
        };
        (digits[4..6].parse().ok()?, digits[6..].parse().ok()?, year)
    };
    let candidate = match year {
        Some(y) => format!("{y:04}-{month:02}-{day:02}"),
        None => format!("--{month:02}-{day:02}"),
    };
    PartialDate::parse(&candidate)
        .filter(PartialDate::is_day)
        .map(PartialDate::to_iso)
}

/// The text of an address from its parts: the street, the extra line and the PO box, then the city,
/// region and postcode on one line, then the country.
pub(super) fn address_value(parts: &Map<String, Value>) -> String {
    let get = |k: &str| {
        parts
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let area = [get("city"), get("region"), get("postal_code")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    [
        get("street"),
        get("extended"),
        get("po_box"),
        area,
        get("country"),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

/// A detail with the ordering hint ("preferred first") the card doesn't keep.
struct Found {
    detail: CardDetail,
    pref: bool,
}

fn plain(kind: &str, label: String, value: String) -> CardDetail {
    CardDetail {
        kind: kind.to_string(),
        label,
        value,
        ..CardDetail::default()
    }
}

/// The dates PM's own export writes as parameters on a job's or school's line (`X-PM-FROM`,
/// `X-PM-TO`, `X-PM-CURRENT=0`). A card from another app has none, so its jobs read as current.
struct PmDates {
    from: Option<String>,
    to: Option<String>,
    current: bool,
}

impl PmDates {
    fn apply(&self, detail: &mut CardDetail) {
        detail.from_date = self.from.clone();
        detail.to_date = self.to.clone();
        detail.is_current = self.current;
    }
}

fn pm_dates(params: &[Param]) -> PmDates {
    PmDates {
        from: param(params, "X-PM-FROM").map(str::to_string),
        to: param(params, "X-PM-TO").map(str::to_string),
        current: param(params, "X-PM-CURRENT") != Some("0"),
    }
}

/// One `ORG` line, waiting to be paired with a title.
struct Org<'a> {
    group: Option<&'a str>,
    company: String,
    dept: String,
    dates: PmDates,
}

/// Map one card's properties to a [`Card`].
fn build_card(props: &[Prop], warnings: &mut BTreeMap<Warning, usize>) -> Built {
    // An item-grouped label (Apple, Google) sits on a sibling property of the same group.
    let mut group_labels: HashMap<&str, String> = HashMap::new();
    for p in props.iter().filter(|p| p.name == "X-GOOGLE-LABEL") {
        if let Some(g) = &p.group {
            let l = apple_label(p.first());
            if !l.is_empty() {
                group_labels.insert(g, l);
            }
        }
    }
    for p in props.iter().filter(|p| p.name == "X-ABLABEL") {
        if let Some(g) = &p.group {
            let l = apple_label(p.first());
            if !l.is_empty() {
                group_labels.insert(g, l);
            }
        }
    }
    let label_of = |p: &Prop, kind: &str| -> String {
        p.group
            .as_deref()
            .and_then(|g| group_labels.get(g))
            .cloned()
            .unwrap_or_else(|| type_label(p, kind))
    };

    let mut card = Card::default();
    let mut found: Vec<Found> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut bday_raw: Option<String> = None;
    let mut bday_ok = false;
    let mut orgs: Vec<Org> = Vec::new();
    let mut titles: Vec<(Option<&str>, String)> = Vec::new();
    let mut roles: Vec<(Option<&str>, String)> = Vec::new();

    for p in props {
        let value = p.first().trim();
        match p.name.as_str() {
            "KIND" => {
                if matches!(value.to_ascii_lowercase().as_str(), "group" | "location") {
                    return Built::NotAPerson;
                }
            }
            "FN" => {
                if card.display_name.is_empty() {
                    card.display_name = value.to_string();
                }
            }
            "N" => {
                let part = |i: usize| p.parts.get(i).map(|s| s.trim().to_string());
                card.family_name = part(0).unwrap_or_default();
                card.given_name = part(1).unwrap_or_default();
                card.middle_name = part(2).unwrap_or_default();
                card.name_prefix = part(3).unwrap_or_default();
                card.name_suffix = part(4).unwrap_or_default();
            }
            "NICKNAME" => {
                for item in p.parts.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    found.push(Found {
                        detail: plain("nickname", String::new(), item.to_string()),
                        pref: false,
                    });
                }
            }
            "CATEGORIES" => {
                for item in p.parts.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    card.labels.push(item.to_string());
                }
            }
            "EMAIL" if !value.is_empty() => found.push(Found {
                detail: plain("email", label_of(p, "email"), value.to_string()),
                pref: p.is_pref(),
            }),
            "TEL" if !value.is_empty() => {
                let number = match value.get(..4) {
                    Some(prefix) if prefix.eq_ignore_ascii_case("tel:") => value[4..].trim(),
                    _ => value,
                };
                if !number.is_empty() {
                    found.push(Found {
                        detail: plain("phone", label_of(p, "phone"), number.to_string()),
                        pref: p.is_pref(),
                    });
                }
            }
            "ADR" => {
                let get = |i: usize| {
                    p.parts
                        .get(i)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default()
                };
                let mut parts = Map::new();
                for (i, key) in [
                    "po_box",
                    "extended",
                    "street",
                    "city",
                    "region",
                    "postal_code",
                    "country",
                ]
                .into_iter()
                .enumerate()
                {
                    let v = get(i);
                    if !v.is_empty() {
                        parts.insert(key.to_string(), Value::String(v));
                    }
                }
                let (value, parts) = if parts.is_empty() {
                    // A 3.0 card may carry the whole address as text in a LABEL parameter.
                    let text = param(&p.params, "LABEL").unwrap_or("").trim().to_string();
                    (text, None)
                } else if parts.len() == 1 && parts.contains_key("street") {
                    (get(2), None)
                } else {
                    (address_value(&parts), Some(Value::Object(parts)))
                };
                if !value.is_empty() {
                    found.push(Found {
                        detail: CardDetail {
                            parts,
                            ..plain("address", label_of(p, "address"), value)
                        },
                        pref: p.is_pref(),
                    });
                }
            }
            "URL" if !value.is_empty() => {
                let label = p
                    .group
                    .as_deref()
                    .and_then(|g| group_labels.get(g))
                    .cloned()
                    .or_else(|| network_of_url(value).map(|n| n.label.to_string()))
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| {
                        let t = type_label(p, "web");
                        if t.is_empty() {
                            "Website".to_string()
                        } else {
                            t
                        }
                    });
                found.push(Found {
                    detail: plain("web", label, value.to_string()),
                    pref: p.is_pref(),
                });
            }
            "X-SOCIALPROFILE" | "SOCIALPROFILE" => {
                let kind = param(&p.params, "SERVICE-TYPE")
                    .or_else(|| param(&p.params, "TYPE"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let handle = param(&p.params, "X-USER")
                    .or_else(|| param(&p.params, "USERNAME"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let value = if value.is_empty() {
                    handle
                } else {
                    value.to_string()
                };
                if value.is_empty() {
                    continue;
                }
                let label = if kind.is_empty() {
                    network_of_url(&value)
                        .map_or_else(|| "Website".to_string(), |n| n.label.to_string())
                } else {
                    social_label(&kind)
                };
                found.push(Found {
                    detail: plain("web", label, value),
                    pref: p.is_pref(),
                });
            }
            "IMPP" if !value.is_empty() => {
                let (scheme, rest) = match value.split_once(':') {
                    Some((s, r)) if !s.is_empty() && !r.is_empty() => (s, r),
                    _ => ("", value),
                };
                let web = matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https");
                let service = param(&p.params, "X-SERVICE-TYPE")
                    .or_else(|| param(&p.params, "SERVICE-TYPE"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let (label, shown) = if web {
                    (
                        network_of_url(value)
                            .map_or_else(|| "Website".to_string(), |n| n.label.to_string()),
                        value,
                    )
                } else {
                    let label = if !service.is_empty() {
                        service
                    } else if scheme.is_empty() {
                        "Messenger".to_string()
                    } else {
                        messenger_label(scheme)
                    };
                    (label, rest)
                };
                found.push(Found {
                    detail: plain("web", label, shown.to_string()),
                    pref: p.is_pref(),
                });
            }
            "X-SKYPE" | "X-AIM" | "X-GOOGLE-TALK" | "X-JABBER" | "X-MSN" | "X-ICQ" | "X-YAHOO"
            | "X-QQ"
                if !value.is_empty() =>
            {
                let label = match p.name.as_str() {
                    "X-SKYPE" => "Skype",
                    "X-AIM" => "AIM",
                    "X-GOOGLE-TALK" => "Google Talk",
                    "X-JABBER" => "Jabber",
                    "X-MSN" => "MSN",
                    "X-ICQ" => "ICQ",
                    "X-YAHOO" => "Yahoo",
                    _ => "QQ",
                };
                found.push(Found {
                    detail: plain("web", label.to_string(), value.to_string()),
                    pref: false,
                });
            }
            "ORG" => {
                let company = p
                    .parts
                    .first()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let dept = p
                    .parts
                    .iter()
                    .skip(1)
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" / ");
                if !company.is_empty() {
                    orgs.push(Org {
                        group: p.group.as_deref(),
                        company,
                        dept,
                        dates: pm_dates(&p.params),
                    });
                }
            }
            // PM's own school, former name and custom field (no vCard property carries them), so
            // that a PM export read back into another PM loses nothing (E12).
            "X-PM-SCHOOL" if !value.is_empty() => {
                let field = p.parts.get(1).map(|s| s.trim()).unwrap_or("");
                let mut detail = CardDetail {
                    parts: (!field.is_empty()).then(|| {
                        Value::Object(Map::from_iter([(
                            "field".to_string(),
                            Value::String(field.to_string()),
                        )]))
                    }),
                    ..plain("school", String::new(), value.to_string())
                };
                pm_dates(&p.params).apply(&mut detail);
                found.push(Found {
                    detail,
                    pref: false,
                });
            }
            "X-PM-FORMER-NAME" if !value.is_empty() => found.push(Found {
                detail: plain("former_name", String::new(), value.to_string()),
                pref: false,
            }),
            "X-PM-CUSTOM" if !value.is_empty() => found.push(Found {
                detail: plain("custom", label_of(p, "custom"), value.to_string()),
                pref: false,
            }),
            "TITLE" if !value.is_empty() => titles.push((p.group.as_deref(), value.to_string())),
            "ROLE" if !value.is_empty() => roles.push((p.group.as_deref(), value.to_string())),
            "BDAY" => {
                if bday_raw.is_none() {
                    bday_raw = Some(value.to_string());
                }
                if !bday_ok && card.born_on.is_none() {
                    if let Some(date) =
                        parse_card_date(value, param(&p.params, "X-APPLE-OMIT-YEAR"))
                    {
                        card.born_on = Some(date);
                        bday_ok = true;
                    }
                }
            }
            "ANNIVERSARY" | "X-ANNIVERSARY" | "X-ABDATE" if !value.is_empty() => {
                let label = if p.name == "X-ABDATE" {
                    label_of(p, "date")
                } else {
                    let l = label_of(p, "date");
                    if l.is_empty() {
                        "Anniversary".to_string()
                    } else {
                        l
                    }
                };
                match parse_card_date(value, param(&p.params, "X-APPLE-OMIT-YEAR")) {
                    Some(date) => found.push(Found {
                        detail: plain("date", label, date),
                        pref: false,
                    }),
                    None => {
                        warn(warnings, Warning::BadDate);
                        let label = if label.is_empty() {
                            "Date (as written)".to_string()
                        } else {
                            label
                        };
                        found.push(Found {
                            detail: plain("custom", label, value.to_string()),
                            pref: false,
                        });
                    }
                }
            }
            "X-ABRELATEDNAMES" | "RELATED" if !value.is_empty() => {
                // A link to another card is not a name.
                let is_link = param(&p.params, "VALUE")
                    .is_some_and(|v| v.eq_ignore_ascii_case("uri"))
                    || value.to_ascii_lowercase().starts_with("urn:");
                if !is_link {
                    found.push(Found {
                        detail: plain("relation", label_of(p, "relation"), value.to_string()),
                        pref: false,
                    });
                }
            }
            "NOTE" if !value.is_empty() => notes.push(p.first().to_string()),
            "UID" => {
                if card.ident.is_none() && !value.is_empty() {
                    card.ident = Some(value.to_string());
                }
            }
            "REV" => {
                if card.rev.is_none() && !value.is_empty() {
                    card.rev = Some(value.to_string());
                }
            }
            "X-PM-MET-WHERE" => card.met_where = value.to_string(),
            "X-PM-MET-ON" => card.met_on = Some(value.to_string()),
            "X-PM-LOCATION" if !value.is_empty() => {
                found.push(Found {
                    detail: CardDetail {
                        from_date: param(&p.params, "X-PM-FROM").map(str::to_string),
                        ..plain("location", String::new(), value.to_string())
                    },
                    pref: false,
                });
            }
            _ => {}
        }
    }

    if !bday_ok {
        if let Some(raw) = bday_raw.filter(|r| !r.is_empty()) {
            warn(warnings, Warning::BadDate);
            found.push(Found {
                detail: plain("custom", "Birthday (as written)".to_string(), raw),
                pref: false,
            });
        }
    }

    // A job: each company takes the title of its own group, else the next one in order.
    let spare_titles = if titles.is_empty() { &roles } else { &titles };
    let mut used = vec![false; spare_titles.len()];
    for Org {
        group,
        company,
        dept,
        dates,
    } in &orgs
    {
        let mut at = group.and_then(|g| {
            spare_titles
                .iter()
                .enumerate()
                .position(|(i, (tg, _))| !used[i] && *tg == Some(g))
        });
        if at.is_none() {
            at = spare_titles
                .iter()
                .enumerate()
                .position(|(i, (tg, _))| !used[i] && tg.is_none());
        }
        let mut parts = Map::new();
        if let Some(i) = at {
            used[i] = true;
            parts.insert("title".into(), Value::String(spare_titles[i].1.clone()));
        }
        if !dept.is_empty() {
            parts.insert("department".into(), Value::String(dept.clone()));
        }
        let mut detail = CardDetail {
            parts: (!parts.is_empty()).then_some(Value::Object(parts)),
            ..plain("work", String::new(), company.clone())
        };
        dates.apply(&mut detail);
        found.push(Found {
            detail,
            pref: false,
        });
    }
    for (i, (_, title)) in spare_titles.iter().enumerate() {
        if !used[i] {
            // A title with no company to go with is not a company: it stays as a plain custom
            // field, so the company column (searched, shown, sent on to Google) never holds "CTO".
            found.push(Found {
                detail: plain("custom", "Job title".to_string(), title.clone()),
                pref: false,
            });
        }
    }

    // Preferred first within a kind (a stable sort keeps the file's order otherwise).
    found.sort_by(|a, b| a.detail.kind.cmp(&b.detail.kind).then(b.pref.cmp(&a.pref)));
    card.details = found.into_iter().map(|f| f.detail).collect();
    card.notes = notes.join("\n\n");
    card.met_on = card.met_on.take().filter(|d| !d.is_empty());

    match card.finish() {
        None => Built::NoName,
        Some(cuts) => {
            if cuts.values > 0 {
                warn(warnings, Warning::ValueCut);
            }
            if cuts.dropped > 0 {
                warn(warnings, Warning::TooManyDetails);
            }
            Built::Card(Box::new(card))
        }
    }
}

/// The label of an instant-messaging URI scheme.
fn messenger_label(scheme: &str) -> String {
    match scheme.to_ascii_lowercase().as_str() {
        "skype" => "Skype".into(),
        "xmpp" => "XMPP".into(),
        "aim" => "AIM".into(),
        "icq" => "ICQ".into(),
        "msn" => "MSN".into(),
        "ymsgr" => "Yahoo".into(),
        "irc" => "IRC".into(),
        "sip" => "SIP".into(),
        "whatsapp" => "WhatsApp".into(),
        _ => title_case(scheme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_in_every_shape_reads_the_same() {
        for (raw, want) in [
            ("1996-04-15", Some("1996-04-15")),
            ("19960415", Some("1996-04-15")),
            ("--04-15", Some("--04-15")),
            ("--0415", Some("--04-15")),
            ("1604-04-15", Some("--04-15")),
            ("0000-04-15", Some("--04-15")),
            ("19900415T000000-0500", Some("1990-04-15")),
            ("1996-04-15T10:00:00Z", Some("1996-04-15")),
            ("1604-02-29", Some("--02-29")),
            ("1999-02-29", None),
            ("---15", None),
            ("1996-04", None),
            ("circa 1800", None),
            ("", None),
        ] {
            assert_eq!(parse_card_date(raw, None).as_deref(), want, "{raw:?}");
        }
        assert_eq!(
            parse_card_date("1900-04-15", Some("1900")).as_deref(),
            Some("--04-15")
        );
    }

    #[test]
    fn apple_wrappers_lose_their_dressing() {
        assert_eq!(apple_label("_$!<Work>!$_"), "work");
        assert_eq!(apple_label("_$!<HomePage>!$_"), "Homepage");
        assert_eq!(apple_label("_$!<Spouse>!$_"), "Spouse");
        assert_eq!(apple_label("  Work  "), "Work");
    }

    #[test]
    fn a_host_is_read_without_its_dressing() {
        assert_eq!(host_of("https://www.LinkedIn.com/in/x?y=1"), "linkedin.com");
        assert_eq!(host_of("http://user@x.com:8080/a"), "x.com");
        assert_eq!(
            network_of_url("https://uk.linkedin.com/in/x")
                .unwrap()
                .label,
            "LinkedIn"
        );
        assert!(network_of_url("https://notlinkedin.com/x").is_none());
    }
}
