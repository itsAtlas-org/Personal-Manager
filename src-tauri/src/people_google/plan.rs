// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The requests PM sends to Google's People API, built without sending them, so their exact shape
//! is tested (spec 7.5, "Testing seam"): the pinned list parameters, the masks, the etag in the
//! body, `updatePersonFields` naming only what changed, and arrays that are Google's own fresh
//! arrays with PM's item changes applied in place.
//!
//! **Resource names** are validated (`people/c` and digits; `contactGroups/` and letters and
//! digits) and appended **without** percent-encoding the slash: the Calendar writer's
//! `is_plain_segment` encodes a segment, which would turn `people/c123` into `people%2Fc123`.
//!
//! **Arrays are replaced whole by Google** (`updatePersonFields` replaces what it lists), so an
//! array is never built from PM's model alone: it starts as Google's fresh array and PM's changes
//! are applied to it item by item, found by their matching key (the item a PM row paired with, by
//! `src_key`). Every attribute PM doesn't model (phonetic names, an address's country code, an
//! email's display name, an organization's cost centre, a type PM has no word for) therefore
//! survives. Singletons (name, birthday, note) never carry a second value.
//!
//! No value is put into an error: [`PlanError`] names the kind of trouble and nothing else.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::people::fold::fold_key;
use crate::people::NOTES_MAX;

use super::classify::Method;
use super::map::{
    self, born_array, finish_name, key_of_item, new_item, notes_array, set_attr, slots,
    MY_CONTACTS_GROUP, READ_MASK, SOURCES, STARRED_GROUP,
};
use super::merge::{Send, Single};

const API: &str = "https://people.googleapis.com/v1";
/// The page size of every list call: pinned, because Google requires every parameter of a sync to
/// match the call that issued its token.
pub const LIST_PAGE_SIZE: u32 = 1000;
/// `people:batchGet` takes at most this many resource names.
pub const BATCH_GET_MAX: usize = 200;
/// What PM reads of a contact group.
const GROUP_FIELDS: &str = "name,groupType,metadata";

/// The HTTP verb of a request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// One request to the People API, ready for the one sender.
#[derive(Clone, PartialEq)]
pub struct RequestPlan {
    pub verb: Verb,
    /// What the classifier should treat the answer as.
    pub method: Method,
    /// The full URL, the resource name's slash intact and the query encoded.
    pub url: String,
    pub body: Option<Value>,
}

impl std::fmt::Debug for RequestPlan {
    /// The kind of request only: the URL and body hold resource names and contact values.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestPlan")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

/// Why a request could not be planned. Never carries a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    /// A resource name that is not `people/c<digits>` or `contactGroups/<id>`.
    BadResource,
    /// More names than one batch get takes.
    TooMany,
    /// The fresh contact carries no CONTACT etag to send.
    MissingEtag,
    /// A label with no Google group yet: the caller creates the group first.
    UnknownLabel,
    /// An edit or a removal names an item Google's fresh copy no longer has: the caller reads the
    /// contact again and merges again, rather than treating the edit as already made. `at` is the
    /// index in `UpdateInput::send` of the change that failed.
    ItemMissing { at: usize },
    /// PM's note would replace a Google note longer than PM keeps, losing its tail. This one does
    /// not clear on a re-read: the caller drops the `Single::Notes` send at `at` (index in
    /// `UpdateInput::send`), raises a notes conflict instead, and plans the other sends again, so
    /// one stuck note never holds back the contact's other pending edits.
    NotesTooLong { at: usize },
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PlanError::BadResource => "not a Google resource name",
            PlanError::TooMany => "too many contacts for one request",
            PlanError::MissingEtag => "the contact has no etag",
            PlanError::UnknownLabel => "a label has no Google group yet",
            PlanError::ItemMissing { .. } => "an item to change is no longer in the contact",
            PlanError::NotesTooLong { .. } => "the contact's note is longer than PM keeps",
        })
    }
}

/// `people/c` and one or more digits: the only contact names PM sends or stores.
pub fn is_contact_resource(name: &str) -> bool {
    name.strip_prefix("people/c")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// `contactGroups/` and one or more letters or digits (a user group's id, or a system group's
/// word such as `myContacts`).
pub fn is_group_resource(name: &str) -> bool {
    name.strip_prefix("contactGroups/")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// `path` under the API root, the slash of a resource name kept, the query encoded.
fn url(path: &str, query: &[(&str, &str)]) -> String {
    let mut u = reqwest::Url::parse(&format!("{API}/{path}"))
        .expect("the People API base with a validated path is a valid URL");
    if !query.is_empty() {
        u.query_pairs_mut().extend_pairs(query);
    }
    u.to_string()
}

fn plan(verb: Verb, method: Method, url: String, body: Option<Value>) -> RequestPlan {
    RequestPlan {
        verb,
        method,
        url,
        body,
    }
}

fn contact(name: &str) -> Result<&str, PlanError> {
    if is_contact_resource(name) {
        Ok(name)
    } else {
        Err(PlanError::BadResource)
    }
}

fn group(name: &str) -> Result<&str, PlanError> {
    if is_group_resource(name) {
        Ok(name)
    } else {
        Err(PlanError::BadResource)
    }
}

// --- reading --------------------------------------------------------------------------------------

/// The list call's pinned parameters as a hash, stored beside the sync token. A token is only
/// valid for the call that issued it, so a changed hash means a full sync.
#[allow(dead_code)] // the sync (g3) stores it beside the token; until then only the tests call it
pub fn list_params_hash() -> String {
    let digest = Sha256::digest(pinned_params().as_bytes());
    hex::encode(digest)[..32].to_string()
}

/// The text the hash is of. Its exact form is stored beside every user's sync token, so changing
/// it (an order, a separator, a parameter) makes every stored hash mismatch and forces a full sync
/// for everyone: the tests pin it letter for letter.
fn pinned_params() -> String {
    format!(
        "pageSize={LIST_PAGE_SIZE};requestSyncToken=true;personFields={READ_MASK};sources={SOURCES}"
    )
}

/// `people.connections.list`: every page of a full sync, or the changes since `sync_token`.
/// `page_token` is the previous page's `nextPageToken`.
#[allow(dead_code)] // the sync (g3) sends it; until then only the tests build it
pub fn list_connections(sync_token: Option<&str>, page_token: Option<&str>) -> RequestPlan {
    let size = LIST_PAGE_SIZE.to_string();
    let mut q = vec![
        ("personFields", READ_MASK),
        ("sources", SOURCES),
        ("pageSize", size.as_str()),
        ("requestSyncToken", "true"),
    ];
    if let Some(t) = sync_token {
        q.push(("syncToken", t));
    }
    if let Some(t) = page_token {
        q.push(("pageToken", t));
    }
    plan(
        Verb::Get,
        Method::List,
        url("people/me/connections", &q),
        None,
    )
}

/// `people.get`: one contact fresh, with the same masks as every other read.
#[allow(dead_code)] // the sync and the outbound pass (g3, g4) send it; until then only the tests build it
pub fn get_person(resource: &str) -> Result<RequestPlan, PlanError> {
    let path = contact(resource)?;
    Ok(plan(
        Verb::Get,
        Method::Get,
        url(path, &[("personFields", READ_MASK), ("sources", SOURCES)]),
        None,
    ))
}

/// `people:batchGet`: up to [`BATCH_GET_MAX`] contacts at once.
#[allow(dead_code)] // the sync (g3) fetches capped contacts whole with it; until then only the tests build it
pub fn batch_get(resources: &[&str]) -> Result<RequestPlan, PlanError> {
    if resources.len() > BATCH_GET_MAX {
        return Err(PlanError::TooMany);
    }
    let mut q: Vec<(&str, &str)> = Vec::with_capacity(resources.len() + 2);
    for r in resources {
        q.push(("resourceNames", contact(r)?));
    }
    q.push(("personFields", READ_MASK));
    q.push(("sources", SOURCES));
    Ok(plan(
        Verb::Get,
        Method::Get,
        url("people:batchGet", &q),
        None,
    ))
}

/// `people/me` for the account's address, with **no `sources`**: the profile applies, because the
/// address is read from the account and nothing from this call is stored or merged.
#[allow(dead_code)] // g1's own sender builds this request today; the shared sender (g3) will use this plan
pub fn me_address() -> RequestPlan {
    plan(
        Verb::Get,
        Method::Get,
        url("people/me", &[("personFields", "emailAddresses")]),
        None,
    )
}

// --- writing a contact ----------------------------------------------------------------------------

/// `createContact` with `body` (see [`map::person_for_create`], which carries the `pm.uid`
/// `clientData`). The reply is read with the same masks as every read.
#[allow(dead_code)] // the outbound pass (g4) sends it; until then only the tests build it
pub fn create_contact(body: Value) -> RequestPlan {
    plan(
        Verb::Post,
        Method::Create,
        url(
            "people:createContact",
            &[("personFields", READ_MASK), ("sources", SOURCES)],
        ),
        Some(body),
    )
}

/// `deleteContact`.
#[allow(dead_code)] // the outbound pass (g4) sends it; until then only the tests build it
pub fn delete_contact(resource: &str) -> Result<RequestPlan, PlanError> {
    let path = format!("{}:deleteContact", contact(resource)?);
    Ok(plan(Verb::Delete, Method::Delete, url(&path, &[]), None))
}

/// The CONTACT source's etag of a fresh contact.
pub fn contact_etag(person: &Value) -> Option<String> {
    person
        .pointer("/metadata/sources")
        .and_then(Value::as_array)?
        .iter()
        .find(|s| s.get("type").and_then(Value::as_str) == Some("CONTACT"))?
        .get("etag")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The fields `updatePersonFields` may list, in the order PM lists them.
const UPDATE_ORDER: [&str; 14] = [
    "names",
    "nicknames",
    "birthdays",
    "events",
    "emailAddresses",
    "phoneNumbers",
    "addresses",
    "organizations",
    "urls",
    "imClients",
    "relations",
    "biographies",
    "userDefined",
    "memberships",
];

/// What an update is built from.
pub struct UpdateInput<'a> {
    /// `people/c<digits>`.
    pub resource: &'a str,
    /// The contact as Google has it now (read with the masks a moment ago).
    pub fresh: &'a Value,
    /// What PM changed and Google lacks (`merge::Outcome::send`): the frozen fields are not in it.
    pub send: &'a [Send],
    /// This account's groups by folded label name, for labels PM adds or removes.
    pub groups: &'a BTreeMap<String, String>,
}

/// Google's fresh arrays with PM's changes applied, as an `updateContact` request. `Ok(None)` when
/// nothing would change (what PM wants is already what Google holds).
#[allow(dead_code)] // the outbound pass (g4) sends it; until then only the tests build it
pub fn update_contact(input: &UpdateInput<'_>) -> Result<Option<RequestPlan>, PlanError> {
    let resource = contact(input.resource)?;
    let etag = contact_etag(input.fresh).ok_or(PlanError::MissingEtag)?;
    let mut work = input.fresh.clone();
    let mut touched: BTreeSet<&'static str> = BTreeSet::new();
    let mut name_sets: Vec<(Single, Option<String>)> = Vec::new();
    let mut star: Option<bool> = None;
    let mut label_adds: Vec<&str> = Vec::new();
    let mut label_removes: Vec<&str> = Vec::new();

    for (at, s) in input.send.iter().enumerate() {
        match s {
            Send::Set(f, v) => match f {
                Single::BornOn => {
                    work["birthdays"] = born_array(v.as_deref());
                    touched.insert("birthdays");
                }
                Single::Notes => {
                    // PM keeps at most NOTES_MAX bytes of a note, so its text is a cut copy of a
                    // longer Google note; writing it back would silently drop the tail.
                    let longer = work
                        .pointer("/biographies/0/value")
                        .and_then(Value::as_str)
                        .is_some_and(|raw| raw.len() > NOTES_MAX);
                    if longer {
                        return Err(PlanError::NotesTooLong { at });
                    }
                    work["biographies"] = notes_array(v.as_deref().unwrap_or(""));
                    touched.insert("biographies");
                }
                Single::Starred => star = Some(v.is_some()),
                name => name_sets.push((*name, v.clone())),
            },
            Send::AddLabel(n) => label_adds.push(n),
            Send::RemoveLabel(n) => label_removes.push(n),
            Send::RemoveItem { kind, google_key } => {
                let slot = locate(&work, kind, google_key).ok_or(PlanError::ItemMissing { at })?;
                if let Some(a) = work[slot.field].as_array_mut() {
                    a.remove(slot.index);
                }
                touched.insert(slot.field);
            }
            Send::EditItem {
                kind,
                google_key,
                attrs,
                to,
                primary,
                ..
            } => {
                let slot = locate(&work, kind, google_key).ok_or(PlanError::ItemMissing { at })?;
                let mut changed = false;
                for attr in attrs {
                    if attr == "primary" {
                        if let Some(a) = work[slot.field].as_array_mut() {
                            changed |= mark_primary(a, slot.index, *primary);
                        }
                    } else {
                        changed |=
                            set_attr(slot.field, &mut work[slot.field][slot.index], attr, to);
                    }
                }
                if changed {
                    touched.insert(slot.field);
                }
            }
            Send::AddItem {
                detail, primary, ..
            } => {
                if let Some((field, item)) = new_item(detail) {
                    let arr = work
                        .as_object_mut()
                        .map(|o| o.entry(field).or_insert_with(|| json!([])));
                    if let Some(Value::Array(a)) = arr {
                        a.push(item);
                        if *primary {
                            let last = a.len() - 1;
                            mark_primary(a, last, true);
                        }
                        touched.insert(field);
                    }
                }
            }
        }
    }

    if !name_sets.is_empty() {
        let before = work.get("names").cloned();
        let existing = work
            .get("names")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .cloned();
        let mut o = existing
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (single, v) in &name_sets {
            let key = match single {
                Single::GivenName => "givenName",
                Single::MiddleName => "middleName",
                Single::FamilyName => "familyName",
                Single::NamePrefix => "honorificPrefix",
                _ => "honorificSuffix",
            };
            match v {
                Some(text) => {
                    o.insert(key.into(), json!(text));
                }
                None => {
                    o.remove(key);
                }
            }
        }
        // The same ending as a create's name (`write_name`): output-only keys go, and a free-form
        // name goes only once a part is set. With nothing left the name is cleared, not sent empty.
        finish_name(&mut o, None);
        work["names"] = if o.is_empty() {
            json!([])
        } else {
            json!([Value::Object(o)])
        };
        if work.get("names") != before.as_ref() {
            touched.insert("names");
        }
    }

    if star.is_some() || !label_adds.is_empty() || !label_removes.is_empty() {
        let mut members = memberships_of(&work);
        let mut changed = false;
        match star {
            Some(true) => changed |= push_unique(&mut members, STARRED_GROUP),
            Some(false) => {
                let n = members.len();
                members.retain(|m| m != STARRED_GROUP);
                changed |= members.len() != n;
            }
            None => {}
        }
        for name in &label_adds {
            let g = input
                .groups
                .get(&fold_key(name))
                .ok_or(PlanError::UnknownLabel)?;
            if !is_group_resource(g) {
                return Err(PlanError::BadResource);
            }
            changed |= push_unique(&mut members, g);
        }
        for name in &label_removes {
            if let Some(g) = input.groups.get(&fold_key(name)) {
                let n = members.len();
                members.retain(|m| m != g);
                changed |= members.len() != n;
            }
        }
        if changed {
            // An update with no membership is refused, and myContacts is never PM's to drop.
            if !members.iter().any(|m| m == MY_CONTACTS_GROUP) {
                members.push(MY_CONTACTS_GROUP.to_string());
            }
            work["memberships"] = Value::Array(
                members
                    .into_iter()
                    .map(|g| json!({ "contactGroupMembership": { "contactGroupResourceName": g } }))
                    .collect(),
            );
            touched.insert("memberships");
        }
    }

    let fields: Vec<&str> = UPDATE_ORDER
        .iter()
        .copied()
        .filter(|f| touched.contains(f))
        .collect();
    if fields.is_empty() {
        return Ok(None);
    }
    let mut body = Map::new();
    body.insert("resourceName".into(), json!(resource));
    body.insert(
        "metadata".into(),
        json!({ "sources": [{ "type": "CONTACT", "etag": etag }] }),
    );
    for f in &fields {
        let arr = match *f {
            "memberships" | "names" | "birthdays" | "biographies" => work[*f].clone(),
            _ => sanitised(work.get(*f)),
        };
        body.insert((*f).to_string(), arr);
    }
    let list = fields.join(",");
    Ok(Some(plan(
        Verb::Patch,
        Method::Update,
        url(
            &format!("{resource}:updateContact"),
            &[
                ("updatePersonFields", list.as_str()),
                ("personFields", READ_MASK),
                ("sources", SOURCES),
            ],
        ),
        Some(Value::Object(body)),
    )))
}

/// The slot of the Google item whose matching key is `key`.
fn locate(person: &Value, kind: &str, key: &str) -> Option<map::Slot> {
    slots(person, kind)
        .into_iter()
        .find(|s| key_of_item(kind, s.field, &person[s.field][s.index]).as_deref() == Some(key))
}

fn push_unique(members: &mut Vec<String>, g: &str) -> bool {
    if members.iter().any(|m| m == g) {
        false
    } else {
        members.push(g.to_string());
        true
    }
}

/// Whether an item is its array's primary one, read or written form.
fn primary_flag(item: &Value) -> bool {
    item.get("metadata")
        .and_then(|m| m.get("primary").or_else(|| m.get("sourcePrimary")))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn set_primary_flag(item: &mut Value, on: bool) {
    if let Some(o) = item.as_object_mut() {
        if on {
            o.insert("metadata".into(), json!({ "sourcePrimary": true }));
        } else {
            o.remove("metadata");
        }
    }
}

/// Make item `index` the primary one of its array (every other item then isn't), or take the flag
/// off it. Returns whether any item's primary flag changed.
fn mark_primary(items: &mut [Value], index: usize, primary: bool) -> bool {
    let before: Vec<bool> = items.iter().map(primary_flag).collect();
    if primary {
        for (i, item) in items.iter_mut().enumerate() {
            set_primary_flag(item, i == index);
        }
    } else if let Some(item) = items.get_mut(index) {
        set_primary_flag(item, false);
    }
    let after: Vec<bool> = items.iter().map(primary_flag).collect();
    before != after
}

/// An array as Google accepts it back: the output-only twins of fields gone, and a primary item
/// stated as `sourcePrimary` (the writable form), so no item's primary flag changes by being sent.
fn sanitised(arr: Option<&Value>) -> Value {
    let Some(items) = arr.and_then(Value::as_array) else {
        return json!([]);
    };
    Value::Array(
        items
            .iter()
            .map(|item| {
                let Some(o) = item.as_object() else {
                    return item.clone();
                };
                let mut o = o.clone();
                for out in ["formattedType", "formattedProtocol", "canonicalForm"] {
                    o.remove(out);
                }
                let primary = o
                    .get("metadata")
                    .and_then(|m| m.get("primary").or_else(|| m.get("sourcePrimary")))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if primary {
                    o.insert("metadata".into(), json!({ "sourcePrimary": true }));
                } else {
                    o.remove("metadata");
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// The contact-group memberships of a person, whatever their type, in order.
fn memberships_of(person: &Value) -> Vec<String> {
    person
        .get("memberships")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    m.pointer("/contactGroupMembership/contactGroupResourceName")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

// --- contact groups -------------------------------------------------------------------------------

/// `contactGroups.list`, with its own sync token.
#[allow(dead_code)] // the label sync (g5) sends it; until then only the tests build it
pub fn list_groups(sync_token: Option<&str>, page_token: Option<&str>) -> RequestPlan {
    let size = LIST_PAGE_SIZE.to_string();
    let mut q = vec![("pageSize", size.as_str()), ("groupFields", GROUP_FIELDS)];
    if let Some(t) = sync_token {
        q.push(("syncToken", t));
    }
    if let Some(t) = page_token {
        q.push(("pageToken", t));
    }
    plan(Verb::Get, Method::List, url("contactGroups", &q), None)
}

/// `contactGroups.create`.
#[allow(dead_code)] // the label sync (g5) sends it; until then only the tests build it
pub fn create_group(name: &str) -> RequestPlan {
    plan(
        Verb::Post,
        Method::Create,
        url("contactGroups", &[]),
        Some(json!({ "contactGroup": { "name": name }, "readGroupFields": GROUP_FIELDS })),
    )
}

/// `contactGroups.update`: a rename, guarded by the group's etag.
#[allow(dead_code)] // the label sync (g5) sends it; until then only the tests build it
pub fn rename_group(resource: &str, etag: &str, name: &str) -> Result<RequestPlan, PlanError> {
    let path = group(resource)?;
    Ok(plan(
        Verb::Put,
        Method::Update,
        url(path, &[]),
        Some(json!({
            "contactGroup": { "resourceName": resource, "etag": etag, "name": name },
            "updateGroupFields": "name",
            "readGroupFields": GROUP_FIELDS,
        })),
    ))
}

/// `contactGroups.delete`, with **`deleteContacts=false` always**: deleting a label never deletes
/// people.
#[allow(dead_code)] // the label sync (g5) sends it; until then only the tests build it
pub fn delete_group(resource: &str) -> Result<RequestPlan, PlanError> {
    let path = group(resource)?;
    Ok(plan(
        Verb::Delete,
        Method::Delete,
        url(path, &[("deleteContacts", "false")]),
        None,
    ))
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
