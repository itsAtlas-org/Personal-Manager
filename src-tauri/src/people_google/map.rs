// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google's `Person` JSON to PM's [`Card`] and back (spec 7.3, "Mapping"; notes 05 section O).
//! Pure: JSON in, values out, no DB and no network.
//!
//! **What is read.** Only the fields of [`READ_MASK`], and of those only what PM models. Google
//! allows one name, so `names[0]` alone is read; a nickname of type `MAIDEN_NAME` is a former name,
//! every other nickname a nickname; an organization of type `school` is a school, anything else a
//! job; `urls` and `imClients` are both `web`, labelled by their `type` or protocol and never by
//! `formattedType` (localised, output only). A contact's data from Google's own profile never
//! arrives: [`SOURCES`] asks for the contact alone.
//!
//! **What is never read.** Phonetic names, an address's country code, an email's display name, an
//! organization's cost centre and everything else PM has no place for stay in Google. The update
//! path ([`super::plan`]) keeps them by editing Google's fresh array in place, so reading them
//! here is not needed and writing them back is never PM's job.
//!
//! The "slots" of a kind are the places in a Person's arrays its details live, in PM's order
//! (primary first inside each array); `plan` finds the Google item behind a PM row through them.
//!
//! Nothing here derives `Debug` for a type holding contact values (spec 8).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::contacts_accounts::PM_UID_CLIENT_DATA_KEY;
use crate::people::card::{flat_parts, truncate_chars, Card, CardDetail};
use crate::people::fold::{clean_multiline, clean_text, value_key};
use crate::people::partial_date::{self, PartialDate};
use crate::people::vcard::address_value;
use crate::people::VALUE_MAX;

use super::plan::{is_contact_resource, is_group_resource};

/// The `personFields` of every read and of the response mask of every write: one const, so a
/// listing, a `people.get`, a batch get and the reply to a create or update always have the same
/// shape (the merge base would otherwise change shape under a sync token).
pub const READ_MASK: &str = "names,nicknames,birthdays,events,emailAddresses,phoneNumbers,addresses,organizations,urls,imClients,relations,biographies,userDefined,memberships,clientData,metadata";

/// The `sources` of every contacts call: the contact's own data, never Google's profile merged in
/// (which would arrive in PM as if the user had typed it, and be written back).
pub const SOURCES: &str = "READ_SOURCE_TYPE_CONTACT";

/// The label of the custom detail a birthday that Google holds only as text becomes.
pub const BIRTHDAY_AS_WRITTEN: &str = "Birthday (as written)";

/// The Google group that is the star.
pub const STARRED_GROUP: &str = "contactGroups/starred";
/// The Google group every contact belongs to; implicit in PM, always kept on a write.
pub const MY_CONTACTS_GROUP: &str = "contactGroups/myContacts";

/// System groups: never a PM label, and their memberships are kept untouched on a write. Only
/// `groupType = USER_CONTACT_GROUP` maps to a label (spec 7.5), and a user group's id is never one
/// of these words.
const SYSTEM_GROUPS: [&str; 8] = [
    "myContacts",
    "starred",
    "friends",
    "family",
    "coworkers",
    "all",
    "chatBuddies",
    "blocked",
];

/// The detail kinds that have a Google field. `location` is PM's own ("where they are now", E8).
pub const MAPPED_KINDS: [&str; 11] = [
    "email",
    "phone",
    "address",
    "web",
    "work",
    "school",
    "nickname",
    "former_name",
    "date",
    "relation",
    "custom",
];

/// The Person arrays a detail kind lives in.
pub fn fields_of(kind: &str) -> &'static [&'static str] {
    match kind {
        "email" => &["emailAddresses"],
        "phone" => &["phoneNumbers"],
        "address" => &["addresses"],
        "web" => &["urls", "imClients"],
        "work" | "school" => &["organizations"],
        "nickname" | "former_name" => &["nicknames"],
        "date" => &["events"],
        "relation" => &["relations"],
        "custom" => &["userDefined"],
        _ => &[],
    }
}

/// The detail kind of one item in a Person array.
fn kind_of(field: &str, item: &Value) -> Option<&'static str> {
    let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
    Some(match field {
        "emailAddresses" => "email",
        "phoneNumbers" => "phone",
        "addresses" => "address",
        "urls" | "imClients" => "web",
        "organizations" => {
            if ty.eq_ignore_ascii_case("school") {
                "school"
            } else {
                "work"
            }
        }
        "nicknames" => {
            if ty == "MAIDEN_NAME" {
                "former_name"
            } else {
                "nickname"
            }
        }
        "events" => "date",
        "relations" => "relation",
        "userDefined" => "custom",
        _ => return None,
    })
}

/// One place a detail lives: the array and the item's index in it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub field: &'static str,
    pub index: usize,
}

fn is_primary(item: &Value) -> bool {
    item.pointer("/metadata/primary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The slots of `kind` in `person`, in PM's order: each array's primary item first (Google's order
/// carries no meaning, W), the rest as they come. Items that map to no value are not slots.
pub fn slots(person: &Value, kind: &str) -> Vec<Slot> {
    let mut out = Vec::new();
    for field in fields_of(kind).iter().copied() {
        let Some(items) = person.get(field).and_then(Value::as_array) else {
            continue;
        };
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|i| !is_primary(&items[*i]));
        for index in order {
            let item = &items[index];
            if kind_of(field, item) == Some(kind) && detail_of(field, item).is_some() {
                out.push(Slot { field, index });
            }
        }
    }
    out
}

fn text_of(item: &Value, key: &str) -> String {
    let (cut, _) = truncate_chars(
        &clean_text(item.get(key).and_then(Value::as_str).unwrap_or("")),
        VALUE_MAX,
    );
    cut
}

/// A Google `Date` as PM's text: `YYYY-MM-DD`, `YYYY-MM`, `YYYY`, or `--MM-DD` when the year is 0 or
/// absent. `None` for a date PM can't hold.
fn date_text(date: &Value) -> Option<String> {
    let n = |k: &str| date.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (year, month, day) = (n("year"), n("month"), n("day"));
    let text = match (year, month, day) {
        (0, 0, _) | (0, _, 0) => return None,
        (0, m, d) => format!("--{m:02}-{d:02}"),
        (y, 0, _) => format!("{y:04}"),
        (y, m, 0) => format!("{y:04}-{m:02}"),
        (y, m, d) => format!("{y:04}-{m:02}-{d:02}"),
    };
    partial_date::normalise_any(&text)
}

/// PM's partial date text as a Google `Date` (year left out when unknown).
pub fn date_value(text: &str) -> Option<Value> {
    let d = PartialDate::parse(text)?;
    let mut o = Map::new();
    if let Some(y) = d.year {
        o.insert("year".into(), json!(y));
    }
    if let Some(m) = d.month {
        o.insert("month".into(), json!(m));
    }
    if let Some(day) = d.day {
        o.insert("day".into(), json!(day));
    }
    Some(Value::Object(o))
}

fn dated(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(date_text)
        .and_then(|t| partial_date::normalise_dated(&t))
}

/// One Person item as a detail, or `None` when it holds no value.
pub fn detail_of(field: &str, item: &Value) -> Option<CardDetail> {
    let kind = kind_of(field, item)?;
    let ty = text_of(item, "type");
    let mut d = CardDetail {
        kind: kind.to_string(),
        ..Default::default()
    };
    match field {
        "emailAddresses" | "phoneNumbers" | "urls" => {
            d.value = text_of(item, "value");
            d.label = ty;
        }
        "imClients" => {
            d.value = text_of(item, "username");
            let protocol = text_of(item, "protocol");
            d.label = if protocol.is_empty() { ty } else { protocol };
        }
        "nicknames" => d.value = text_of(item, "value"),
        "relations" => {
            d.value = text_of(item, "person");
            d.label = ty;
        }
        "userDefined" => {
            d.value = text_of(item, "value");
            // The word `custom_key` writes for a PM field with no label reads back as no label, so
            // an unlabelled field settles instead of looking edited on every pass.
            let key = text_of(item, "key");
            d.label = if key == custom_key("") {
                String::new()
            } else {
                key
            };
        }
        "events" => {
            d.value = item.get("date").and_then(date_text)?;
            d.label = ty;
        }
        "addresses" => {
            let mut parts = Map::new();
            for (pm, google) in ADDRESS_PARTS {
                let v = clean_multiline(item.get(google).and_then(Value::as_str).unwrap_or(""));
                if !v.is_empty() {
                    parts.insert(pm.to_string(), Value::String(v));
                }
            }
            d.label = ty;
            if parts.is_empty() {
                let (cut, _) = truncate_chars(
                    &clean_multiline(
                        item.get("formattedValue")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    ),
                    VALUE_MAX,
                );
                d.value = cut;
            } else {
                d.value = address_value(&parts);
                d.parts = flat_parts("address", Some(&Value::Object(parts)));
            }
        }
        "organizations" => {
            d.value = text_of(item, "name");
            let mut parts = Map::new();
            // PM keeps a school's field of study in `parts.field` (editor, vCard, dedupe); Google
            // only has `title` for it, and no department.
            let pairs: &[(&str, &str)] = if kind == "school" {
                &[("field", "title")]
            } else {
                &[("title", "title"), ("department", "department")]
            };
            for (pm, google) in pairs.iter().copied() {
                let v = text_of(item, google);
                if !v.is_empty() {
                    parts.insert(pm.to_string(), Value::String(v));
                }
            }
            d.parts = (!parts.is_empty()).then_some(Value::Object(parts));
            d.from_date = dated(item, "startDate");
            d.to_date = dated(item, "endDate");
            d.is_current = item
                .get("current")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            d.label = if ty.eq_ignore_ascii_case("work") || ty.eq_ignore_ascii_case("school") {
                String::new()
            } else {
                ty
            };
        }
        _ => return None,
    }
    if d.value.is_empty() {
        return None;
    }
    Some(d)
}

/// PM's address part names and Google's.
pub const ADDRESS_PARTS: [(&str, &str); 7] = [
    ("po_box", "poBox"),
    ("extended", "extendedAddress"),
    ("street", "streetAddress"),
    ("city", "city"),
    ("region", "region"),
    ("postal_code", "postalCode"),
    ("country", "country"),
];

/// The matching key of the Google item at `slot`: the one the merge and the update pair by.
pub fn key_of_item(kind: &str, field: &str, item: &Value) -> Option<String> {
    let d = detail_of(field, item)?;
    Some(value_key(kind, &d.value, d.parts.as_ref()))
}

/// An address's parts as the merge compares them: its `parts`, or for an address that is one line
/// the street, so a street PM holds flat and one Google holds in `streetAddress` are the same
/// part. A free-text address of several lines has none.
pub fn address_parts(d: &CardDetail) -> BTreeMap<String, String> {
    if let Some(o) = d.parts.as_ref().and_then(Value::as_object) {
        return o
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .filter(|(_, v)| !v.is_empty())
            .collect();
    }
    let mut m = BTreeMap::new();
    if !d.value.is_empty() && !d.value.contains('\n') {
        m.insert("street".to_string(), d.value.clone());
    }
    m
}

/// The address with one part set (or, for an empty `text`, removed), its text recomposed and a
/// street-only address kept flat, the way a file and Google both read one.
pub fn address_with(d: &CardDetail, key: &str, text: &str) -> CardDetail {
    let mut parts = address_parts(d);
    if text.is_empty() {
        parts.remove(key);
    } else {
        parts.insert(key.to_string(), text.to_string());
    }
    let mut out = d.clone();
    if parts.is_empty() {
        out.parts = None;
        return out;
    }
    let mut obj = Map::new();
    for (pm, _) in ADDRESS_PARTS {
        if let Some(v) = parts.get(pm) {
            obj.insert(pm.to_string(), Value::String(v.clone()));
        }
    }
    out.value = address_value(&obj);
    out.parts = flat_parts("address", Some(&Value::Object(obj)));
    out
}

// --- notes ------------------------------------------------------------------------------------

/// Plain text from the HTML Google may return for a note: tags dropped, block ends and `<br>` as
/// line breaks, script and style content gone, common entities decoded.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..open]));
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            // An unclosed "<": the rest is text.
            out.push_str(&decode_entities(&rest[open..]));
            rest = "";
            break;
        };
        let tag = &after[..close];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        rest = &after[close + 1..];
        match name.as_str() {
            "script" | "style" if !closing => {
                let needle = format!("</{name}");
                match rest.to_ascii_lowercase().find(&needle) {
                    Some(at) => {
                        let tail = &rest[at..];
                        rest = tail.find('>').map(|i| &tail[i + 1..]).unwrap_or("");
                    }
                    None => rest = "",
                }
            }
            "br" => out.push('\n'),
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if closing => {
                out.push('\n')
            }
            _ => {}
        }
    }
    out.push_str(&decode_entities(rest));
    clean_multiline(&out)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// --- reading a Person ----------------------------------------------------------------------------

/// What one Google Person is, for the callers of the sync: its ids, its state, and its card.
#[allow(dead_code)] // g3's sync reads every field (ids, etag, groups, the cut flag); until then only the tests do
pub struct Mapped {
    /// `people/c<digits>`.
    pub resource_name: String,
    /// The CONTACT source's id: stable across a change of resource name.
    pub contact_id: Option<String>,
    /// The CONTACT source's etag: the one PM sends and stores.
    pub etag: Option<String>,
    /// `metadata.previousResourceNames`: a contact whose resource name changed.
    pub previous_names: Vec<String>,
    /// `metadata.deleted`: an entry in an incremental listing for a contact that is gone.
    pub deleted: bool,
    /// The `pm.uid` PM put on the contact when it created it.
    pub ident: Option<String>,
    /// The contact as a card; `None` for a deleted entry, and for a contact with nothing to call
    /// it by (no name, nickname, company, email or phone).
    pub card: Option<Card>,
    /// Whether the contact is in Google's Starred group.
    pub starred: bool,
    /// The user groups the contact is in (resource names); the caller resolves them to labels
    /// through `label_sources`. `myContacts` and every other system group are left out.
    pub groups: Vec<String>,
    /// A listed contact with exactly 100 emails or numbers may have been cut (the list call caps
    /// at 100): fetch it with `people.get` before merging, and never merge a capped copy.
    pub may_be_cut: bool,
}

/// The card's labels are group names, which a Person doesn't carry: the caller fills
/// `Card::labels` from `groups` before a merge.
#[allow(dead_code)] // the sync (g3) reads every listed contact through it; until then only the tests do
pub fn read_person(person: &Value) -> Option<Mapped> {
    let resource_name = person.get("resourceName")?.as_str()?.to_string();
    if !is_contact_resource(&resource_name) {
        return None;
    }
    let sources = person
        .pointer("/metadata/sources")
        .and_then(Value::as_array);
    let contact = sources.and_then(|s| {
        s.iter()
            .find(|x| x.get("type").and_then(Value::as_str) == Some("CONTACT"))
    });
    let str_of = |v: Option<&Value>, k: &str| {
        v.and_then(|v| v.get(k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let previous_names = person
        .pointer("/metadata/previousResourceNames")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|n| is_contact_resource(n))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let deleted = person
        .pointer("/metadata/deleted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ident = person
        .get("clientData")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter().find_map(|c| {
                (c.get("key").and_then(Value::as_str) == Some(PM_UID_CLIENT_DATA_KEY))
                    .then(|| c.get("value").and_then(Value::as_str))
                    .flatten()
            })
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut starred = false;
    let mut groups = Vec::new();
    for m in person
        .get("memberships")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(name) = m
            .pointer("/contactGroupMembership/contactGroupResourceName")
            .and_then(Value::as_str)
        else {
            continue;
        };
        if name == STARRED_GROUP {
            starred = true;
        } else if is_group_resource(name)
            && !is_system_group(name)
            && !groups.iter().any(|g| g == name)
        {
            groups.push(name.to_string());
        }
    }

    let count = |k: &str| {
        person
            .get(k)
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    };
    let may_be_cut = count("emailAddresses") >= 100 || count("phoneNumbers") >= 100;

    let card = if deleted {
        None
    } else {
        card_of_person(person, ident.clone())
    };
    Some(Mapped {
        resource_name,
        contact_id: str_of(contact, "id"),
        etag: str_of(contact, "etag"),
        previous_names,
        deleted,
        ident,
        card,
        starred,
        groups,
        may_be_cut,
    })
}

/// Whether a group resource name is one of Google's own groups.
pub fn is_system_group(name: &str) -> bool {
    name.strip_prefix("contactGroups/")
        .is_some_and(|id| SYSTEM_GROUPS.contains(&id))
}

fn card_of_person(person: &Value, ident: Option<String>) -> Option<Card> {
    let mut card = Card {
        ident,
        ..Default::default()
    };
    if let Some(name) = person
        .get("names")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        card.given_name = text_of(name, "givenName");
        card.middle_name = text_of(name, "middleName");
        card.family_name = text_of(name, "familyName");
        card.name_prefix = text_of(name, "honorificPrefix");
        card.name_suffix = text_of(name, "honorificSuffix");
        let has_parts = [
            &card.given_name,
            &card.middle_name,
            &card.family_name,
            &card.name_prefix,
            &card.name_suffix,
        ]
        .iter()
        .any(|p| !p.is_empty());
        if !has_parts {
            // A name Google holds as one free string: the whole name is the display name.
            card.display_name = text_of(name, "unstructuredName");
        }
    }

    let born = person
        .get("birthdays")
        .and_then(Value::as_array)
        .and_then(|a| a.first());
    if let Some(born) = born {
        let from_date = born
            .get("date")
            .and_then(date_text)
            .and_then(|t| partial_date::normalise_born_on(&t));
        card.born_on = from_date;
        if card.born_on.is_none() {
            // An old contact may hold the birthday only as text (the field is deprecated): kept as a
            // detail, never dropped, and never written back to Google.
            let text = text_of(born, "text");
            if !text.is_empty() {
                card.details.push(CardDetail {
                    kind: "custom".into(),
                    label: BIRTHDAY_AS_WRITTEN.into(),
                    value: text,
                    ..Default::default()
                });
            }
        }
    }

    if let Some(bio) = person
        .get("biographies")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        let raw = bio.get("value").and_then(Value::as_str).unwrap_or("");
        card.notes = if bio.get("contentType").and_then(Value::as_str) == Some("TEXT_HTML") {
            strip_html(raw)
        } else {
            clean_multiline(raw)
        };
    }

    for kind in MAPPED_KINDS {
        for slot in slots(person, kind) {
            let item = &person[slot.field][slot.index];
            if let Some(d) = detail_of(slot.field, item) {
                card.details.push(d);
            }
        }
    }
    card.finish()?;
    Some(card)
}

// --- writing: items and attributes ----------------------------------------------------------------

fn put(o: &mut Map<String, Value>, key: &str, text: &str) {
    if !text.is_empty() {
        o.insert(key.to_string(), Value::String(text.to_string()));
    }
}

/// The new Google item for a detail PM added, and the array it goes in. `None` for a detail that
/// has no Google field (where they are now) or can't be written (the birthday kept as text).
pub fn new_item(d: &CardDetail) -> Option<(&'static str, Value)> {
    let mut o = Map::new();
    let field = match d.kind.as_str() {
        "email" => {
            put(&mut o, "value", &d.value);
            put(&mut o, "type", &d.label);
            "emailAddresses"
        }
        "phone" => {
            put(&mut o, "value", &d.value);
            put(&mut o, "type", &d.label);
            "phoneNumbers"
        }
        "web" => {
            put(&mut o, "value", &d.value);
            put(&mut o, "type", &d.label);
            "urls"
        }
        "address" => {
            match d.parts.as_ref().and_then(Value::as_object) {
                Some(parts) => {
                    for (pm, google) in ADDRESS_PARTS {
                        if let Some(v) = parts.get(pm).and_then(Value::as_str) {
                            put(&mut o, google, v);
                        }
                    }
                }
                // A street on its own line is the street; text over several lines is the whole.
                None if d.value.contains('\n') => put(&mut o, "formattedValue", &d.value),
                None => put(&mut o, "streetAddress", &d.value),
            }
            put(&mut o, "type", &d.label);
            "addresses"
        }
        "work" | "school" => {
            put(&mut o, "name", &d.value);
            let pairs: &[(&str, &str)] = if d.kind == "school" {
                &[("field", "title")]
            } else {
                &[("title", "title"), ("department", "department")]
            };
            for (pm, google) in pairs.iter().copied() {
                if let Some(v) = d
                    .parts
                    .as_ref()
                    .and_then(|p| p.get(pm))
                    .and_then(Value::as_str)
                {
                    put(&mut o, google, v);
                }
            }
            if let Some(v) = d.from_date.as_deref().and_then(date_value) {
                o.insert("startDate".into(), v);
            }
            if let Some(v) = d.to_date.as_deref().and_then(date_value) {
                o.insert("endDate".into(), v);
            }
            if d.is_current {
                o.insert("current".into(), Value::Bool(true));
            }
            // A school must be typed `school` or Google (and PM, reading it back) takes it for a job.
            let ty = if d.kind == "school" {
                "school"
            } else if d.label.is_empty() {
                "work"
            } else {
                d.label.as_str()
            };
            put(&mut o, "type", ty);
            "organizations"
        }
        "nickname" | "former_name" => {
            put(&mut o, "value", &d.value);
            let ty = if d.kind == "former_name" {
                "MAIDEN_NAME"
            } else {
                "DEFAULT"
            };
            put(&mut o, "type", ty);
            "nicknames"
        }
        "date" => {
            o.insert("date".into(), date_value(&d.value)?);
            put(&mut o, "type", &d.label);
            "events"
        }
        "relation" => {
            put(&mut o, "person", &d.value);
            put(&mut o, "type", &d.label);
            "relations"
        }
        "custom" => {
            if d.label == BIRTHDAY_AS_WRITTEN {
                return None;
            }
            put(&mut o, "key", custom_key(&d.label));
            put(&mut o, "value", &d.value);
            "userDefined"
        }
        _ => return None,
    };
    Some((field, Value::Object(o)))
}

/// Write one attribute of a detail onto an existing Google item, leaving every other key of the
/// item (phonetic names, an email's display name, a cost centre, a country code, an unknown type)
/// exactly as Google sent it. `attr` is one of the attribute names of [`super::merge`]. Returns
/// whether the item changed.
pub fn set_attr(field: &str, item: &mut Value, attr: &str, to: &CardDetail) -> bool {
    let Some(o) = item.as_object_mut() else {
        return false;
    };
    // Each write drops the output-only twin that would now disagree.
    let before = Value::Object(o.clone());
    match (field, attr) {
        (_, "primary") => return false, // handled by the caller across the whole array
        ("emailAddresses" | "urls", "value") => {
            o.insert("value".into(), json!(to.value));
        }
        ("phoneNumbers", "value") => {
            o.insert("value".into(), json!(to.value));
            o.remove("canonicalForm");
        }
        ("emailAddresses" | "phoneNumbers" | "urls" | "addresses" | "relations", "label") => {
            set_or_drop(o, "type", &to.label);
            o.remove("formattedType");
        }
        ("imClients", "value") => {
            o.insert("username".into(), json!(to.value));
        }
        ("imClients", "label") => {
            set_or_drop(o, "protocol", &to.label);
            o.remove("formattedProtocol");
        }
        ("nicknames", "value") => {
            o.insert("value".into(), json!(to.value));
        }
        ("relations", "value") => {
            o.insert("person".into(), json!(to.value));
        }
        ("userDefined", "value") => {
            o.insert("value".into(), json!(to.value));
        }
        ("userDefined", "label") => {
            // The same word `new_item` writes for an empty label: Google refuses an empty key.
            o.insert("key".into(), json!(custom_key(&to.label)));
        }
        ("events", "value") => {
            if let Some(d) = date_value(&to.value) {
                o.insert("date".into(), d);
            }
        }
        ("events", "label") => {
            set_or_drop(o, "type", &to.label);
            o.remove("formattedType");
        }
        ("addresses", "value") => {
            // A value change with no parts is the free-text address.
            if to.parts.is_none() {
                for (_, google) in ADDRESS_PARTS {
                    o.remove(google);
                }
                if to.value.contains('\n') {
                    o.insert("formattedValue".into(), json!(to.value));
                } else {
                    o.remove("formattedValue");
                    o.insert("streetAddress".into(), json!(to.value));
                }
            }
        }
        ("organizations", "value") => {
            o.insert("name".into(), json!(to.value));
        }
        ("organizations", "label") => {
            let is_school = o
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.eq_ignore_ascii_case("school"));
            // A school keeps its type; a job's label is its type, empty meaning plain `work`.
            if !is_school {
                let ty = if to.label.is_empty() {
                    "work"
                } else {
                    &to.label
                };
                o.insert("type".into(), json!(ty));
            }
        }
        ("organizations", "from") => set_date(o, "startDate", to.from_date.as_deref()),
        ("organizations", "to") => set_date(o, "endDate", to.to_date.as_deref()),
        ("organizations", "current") => {
            o.insert("current".into(), Value::Bool(to.is_current));
        }
        (_, a) if a.starts_with("parts.") => {
            let key = &a["parts.".len()..];
            let new = if field == "addresses" {
                address_parts(to).get(key).cloned().unwrap_or_default()
            } else {
                to.parts
                    .as_ref()
                    .and_then(|p| p.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let new = new.as_str();
            match field {
                "organizations" if key == "title" || key == "department" => {
                    set_or_drop(o, key, new);
                }
                // A school's field of study is Google's `title`.
                "organizations" if key == "field" => set_or_drop(o, "title", new),
                "addresses" => {
                    if let Some((_, google)) = ADDRESS_PARTS.iter().find(|(pm, _)| *pm == key) {
                        set_or_drop(o, google, new);
                        // Google rebuilds the one-line form from the parts when it is unset.
                        o.remove("formattedValue");
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    Value::Object(o.clone()) != before
}

/// The key a custom field is written under: its label, or a plain word when PM's has none.
fn custom_key(label: &str) -> &str {
    if label.is_empty() {
        "Custom"
    } else {
        label
    }
}

fn set_or_drop(o: &mut Map<String, Value>, key: &str, text: &str) {
    if text.is_empty() {
        o.remove(key);
    } else {
        o.insert(key.to_string(), json!(text));
    }
}

fn set_date(o: &mut Map<String, Value>, key: &str, iso: Option<&str>) {
    match iso.and_then(date_value) {
        Some(d) => {
            o.insert(key.to_string(), d);
        }
        None => {
            o.remove(key);
        }
    }
}

/// `names[0]`'s PM parts written onto an existing name object (or a new one): only the five
/// parts PM models, so phonetic names and the rest stay. Structured parts and a free-form name
/// would disagree, so the free-form one goes whenever a part is set.
pub fn write_name(existing: Option<&Value>, card: &Card) -> Value {
    let mut o = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (google, text) in [
        ("givenName", &card.given_name),
        ("middleName", &card.middle_name),
        ("familyName", &card.family_name),
        ("honorificPrefix", &card.name_prefix),
        ("honorificSuffix", &card.name_suffix),
    ] {
        set_or_drop(&mut o, google, text);
    }
    finish_name(&mut o, Some(&card.display_name));
    Value::Object(o)
}

/// The rule every name write ends with, shared by a create and an update so they never differ:
/// the output-only keys go (Google computes them again), a free-form name goes whenever a part is
/// set (the two would disagree), and with no part left a free-form name stays, or is `display_name`
/// when one is given and the name has none.
pub fn finish_name(o: &mut Map<String, Value>, display_name: Option<&str>) {
    o.remove("displayName");
    o.remove("displayNameLastFirst");
    o.remove("metadata");
    let has_parts = [
        "givenName",
        "middleName",
        "familyName",
        "honorificPrefix",
        "honorificSuffix",
    ]
    .iter()
    .any(|k| o.contains_key(*k));
    if has_parts {
        o.remove("unstructuredName");
    } else if let Some(n) = display_name.filter(|n| !n.is_empty()) {
        o.insert("unstructuredName".into(), json!(n));
    }
}

/// The `birthdays` array for a birthday (an empty array clears it).
pub fn born_array(born_on: Option<&str>) -> Value {
    match born_on.and_then(date_value) {
        Some(d) => json!([{ "date": d }]),
        None => json!([]),
    }
}

/// The `biographies` array for a note (an empty array clears it). Always plain text.
pub fn notes_array(notes: &str) -> Value {
    if notes.is_empty() {
        json!([])
    } else {
        json!([{ "value": notes, "contentType": "TEXT_PLAIN" }])
    }
}

/// A new contact's body: everything a card holds that Google has a field for, the group
/// memberships given (always `myContacts`, the star when set), and the `clientData` that lets a
/// lost create be found again. `groups` are the resource names of the card's labels in this
/// account.
#[allow(dead_code)] // the outbound pass (g4) builds every create with it; until then only the tests do
pub fn person_for_create(card: &Card, starred: bool, groups: &[String], ident: &str) -> Value {
    let mut p = Map::new();
    let any_part = [
        &card.given_name,
        &card.middle_name,
        &card.family_name,
        &card.name_prefix,
        &card.name_suffix,
    ]
    .iter()
    .any(|s| !s.is_empty());
    if any_part || !card.display_name.is_empty() {
        p.insert("names".into(), json!([write_name(None, card)]));
    }
    if card.born_on.is_some() {
        p.insert("birthdays".into(), born_array(card.born_on.as_deref()));
    }
    if !card.notes.is_empty() {
        p.insert("biographies".into(), notes_array(&card.notes));
    }
    for d in &card.details {
        if let Some((field, item)) = new_item(d) {
            if let Value::Array(a) = p.entry(field).or_insert_with(|| json!([])) {
                a.push(item);
            }
        }
    }
    let mut member = vec![
        json!({ "contactGroupMembership": { "contactGroupResourceName": MY_CONTACTS_GROUP } }),
    ];
    if starred {
        member.push(
            json!({ "contactGroupMembership": { "contactGroupResourceName": STARRED_GROUP } }),
        );
    }
    for g in groups {
        member.push(json!({ "contactGroupMembership": { "contactGroupResourceName": g } }));
    }
    p.insert("memberships".into(), Value::Array(member));
    p.insert(
        "clientData".into(),
        json!([{ "key": PM_UID_CLIENT_DATA_KEY, "value": ident }]),
    );
    Value::Object(p)
}

// --- the stored snapshot ---------------------------------------------------------------------------

/// One entry of a snapshot's `sources`: the shape `people::sources` reads the CONTACT id from.
#[derive(Clone, Serialize, Deserialize)]
pub struct SnapSource {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

/// The copy of a Google contact PM keeps in `person_sources.snapshot`: the merge base. A card, the
/// star, and the CONTACT source id (so a tombstone can keep it, spec 2.2). Holds contact values:
/// no `Debug`.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Snap {
    pub card: Card,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub sources: Vec<SnapSource>,
}

#[allow(dead_code)] // the sync (g3) stores and reads every snapshot through these; until then only the tests do
impl Snap {
    /// The snapshot of a mapped contact, with its labels (names) filled in by the caller. `None`
    /// for a contact with no card.
    pub fn of(mapped: &Mapped, labels: Vec<String>) -> Option<Snap> {
        let mut card = mapped.card.clone()?;
        card.labels = labels;
        Some(Snap {
            card,
            starred: mapped.starred,
            sources: mapped
                .contact_id
                .iter()
                .map(|id| SnapSource {
                    kind: "CONTACT".into(),
                    id: id.clone(),
                })
                .collect(),
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// `None` for a damaged snapshot (the caller treats the source as having no base).
    pub fn parse(text: &str) -> Option<Snap> {
        serde_json::from_str(text).ok()
    }
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
