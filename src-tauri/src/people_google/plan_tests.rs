// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The requests, exactly: urls with the slash kept, the pinned masks, the etag in the body, and
//! updates that keep what PM doesn't model. Updates are tested end to end through the merge, so a
//! `Send` the engine produces is the one the plan consumes. Every contact here is invented.

use super::*;
use crate::people::card::{Card, CardDetail};
use crate::people::fold::value_key;
use crate::people_google::map::read_person;
use crate::people_google::merge::{merge, Side};

fn query(plan: &RequestPlan) -> Vec<(String, String)> {
    reqwest::Url::parse(&plan.url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn q<'a>(pairs: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    pairs
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .collect()
}

/// A contact with Google-only attributes in every array PM edits.
fn fresh() -> Value {
    json!({
        "resourceName": "people/c1234567890",
        "metadata": { "sources": [{ "type": "CONTACT", "id": "1234567890", "etag": "etag-7" }] },
        "names": [{ "givenName": "Sam", "familyName": "Example", "phoneticGivenName": "Sahm",
                    "displayName": "Sam Example", "unstructuredName": "Sam Example" }],
        "birthdays": [{ "date": { "month": 4, "day": 15 } }],
        "emailAddresses": [
            { "value": "sam@example.com", "type": "work", "formattedType": "Work",
              "displayName": "Sam at work", "metadata": { "primary": true, "source": { "type": "CONTACT" } } },
            { "value": "sam.home@example.com", "type": "home", "displayName": "Sam at home" }
        ],
        "phoneNumbers": [
            { "value": "+44 7700 900101", "type": "mobile", "canonicalForm": "+447700900101" }
        ],
        "addresses": [{ "streetAddress": "1 Example Road", "city": "Sampleton", "postalCode": "EX1 2AB",
                        "poBox": "PO 7", "country": "Exland", "countryCode": "XX", "type": "home",
                        "formattedValue": "old text" }],
        "organizations": [{ "name": "Example Works", "title": "Engineer", "department": "Tools",
                            "type": "work", "costCenter": "CC-9", "phoneticName": "Egg-zample" }],
        "urls": [{ "value": "https://example.com/sam", "type": "homePage" }],
        "imClients": [{ "username": "sam#0001", "protocol": "Discord" }],
        "biographies": [{ "value": "Met at the fair.", "contentType": "TEXT_PLAIN" }],
        "memberships": [
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/myContacts" } },
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/friends" } },
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/6a2b3c4d5e" } },
            { "domainMembership": { "inViewerDomain": true } }
        ]
    })
}

fn groups() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("climbing".to_string(), "contactGroups/aa11".to_string()),
        (
            "friends club".to_string(),
            "contactGroups/6a2b3c4d5e".to_string(),
        ),
    ])
}

/// What PM sends after editing `base` into `pm`: the merge's sends, planned against `fresh`.
/// Each PM row remembers the key of the Google item it last agreed with.
/// The edit gets PM's card and the `src_key` of each of its rows (parallel to its details): a test
/// that adds, removes or moves a row moves its `src_key` with it.
fn send_plan(fresh: &Value, edit: impl FnOnce(&mut Card, &mut Vec<String>)) -> Option<RequestPlan> {
    let base = read_person(fresh).unwrap().card.unwrap();
    let mut pm = base.clone();
    let mut src: Vec<String> = base
        .details
        .iter()
        .map(|d| value_key(&d.kind, &d.value, d.parts.as_ref()))
        .collect();
    edit(&mut pm, &mut src);
    let plain = |c| Side {
        card: c,
        starred: false,
        src_keys: &[],
    };
    let mine = Side {
        card: &pm,
        starred: false,
        src_keys: &src,
    };
    let outcome = merge(Some(&plain(&base)), &mine, &plain(&base), &[], "now");
    assert!(outcome.conflicts.is_empty() && outcome.apply.is_empty());
    let g = groups();
    update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh,
        send: &outcome.send,
        groups: &g,
    })
    .unwrap()
}

fn row(card: &Card, kind: &str, n: usize) -> usize {
    card.details
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == kind)
        .nth(n)
        .map(|(i, _)| i)
        .unwrap()
}

// --- urls and masks ---------------------------------------------------------------------------------------------

#[test]
fn plan_keeps_the_slash_in_a_resource_name() {
    let get = get_person("people/c123").unwrap();
    assert!(
        get.url
            .starts_with("https://people.googleapis.com/v1/people/c123?"),
        "{}",
        get.url
    );
    assert!(!get.url.contains("%2F") && !get.url.contains("%2f"));
    let del = delete_contact("people/c123").unwrap();
    assert_eq!(
        del.url,
        "https://people.googleapis.com/v1/people/c123:deleteContact"
    );
    assert!(del.verb == Verb::Delete && del.method == Method::Delete);
    let upd = update_contact(&UpdateInput {
        resource: "people/c123",
        fresh: &json!({ "metadata": { "sources": [{ "type": "CONTACT", "etag": "e" }] }, "biographies": [] }),
        send: &[Send::Set(Single::Notes, Some("hi".into()))],
        groups: &groups(),
    })
    .unwrap()
    .unwrap();
    assert!(
        upd.url
            .starts_with("https://people.googleapis.com/v1/people/c123:updateContact?"),
        "{}",
        upd.url
    );
    let grp = delete_group("contactGroups/aa11").unwrap();
    assert!(
        grp.url
            .starts_with("https://people.googleapis.com/v1/contactGroups/aa11?"),
        "{}",
        grp.url
    );
}

#[test]
fn only_plain_resource_names_are_planned() {
    for bad in [
        "people/c",
        "people/c12a",
        "people/12",
        "people/c1/x",
        "people/c1?x=1",
        "people/c1:deleteContact",
        "../people/c1",
        "people/c1%2F",
        "people/me",
        "",
        "contactGroups/aa11",
        " people/c1",
    ] {
        assert!(get_person(bad).is_err(), "{bad:?}");
        assert!(delete_contact(bad).is_err(), "{bad:?}");
        assert!(!is_contact_resource(bad), "{bad:?}");
    }
    for bad in [
        "contactGroups/",
        "contactGroups/a/b",
        "contactGroups/a b",
        "people/c1",
        "contactGroups/..",
        "contactGroups/a?b",
    ] {
        assert!(delete_group(bad).is_err(), "{bad:?}");
        assert!(rename_group(bad, "e", "n").is_err(), "{bad:?}");
        assert!(!is_group_resource(bad), "{bad:?}");
    }
    assert!(
        is_contact_resource("people/c0123456789") && is_group_resource("contactGroups/myContacts")
    );
    assert!(batch_get(&["people/c1", "people/x"]).is_err());
}

#[test]
fn every_read_carries_the_same_masks() {
    let masked = |p: &RequestPlan| {
        let pairs = query(p);
        assert_eq!(q(&pairs, "personFields"), [READ_MASK]);
        assert_eq!(q(&pairs, "sources"), [SOURCES]);
    };
    masked(&list_connections(None, None));
    masked(&get_person("people/c1").unwrap());
    masked(&batch_get(&["people/c1", "people/c2"]).unwrap());
    masked(&create_contact(json!({})));
    let upd = update_contact(&UpdateInput {
        resource: "people/c1",
        fresh: &json!({ "metadata": { "sources": [{ "type": "CONTACT", "etag": "e" }] } }),
        send: &[Send::Set(Single::Notes, Some("x".into()))],
        groups: &groups(),
    })
    .unwrap()
    .unwrap();
    masked(&upd);
}

#[test]
fn the_list_call_is_pinned_and_pages_and_tokens_ride_along() {
    let first = query(&list_connections(None, None));
    assert_eq!(q(&first, "pageSize"), ["1000"]);
    assert_eq!(q(&first, "requestSyncToken"), ["true"]);
    assert!(q(&first, "syncToken").is_empty() && q(&first, "pageToken").is_empty());
    let next = list_connections(Some("sync tok/1"), Some("page+2"));
    let pairs = query(&next);
    assert_eq!(q(&pairs, "syncToken"), ["sync tok/1"]);
    assert_eq!(q(&pairs, "pageToken"), ["page+2"]);
    // Same pinned parameters on every call of one sync.
    assert_eq!(q(&pairs, "pageSize"), ["1000"]);
    assert!(next.verb == Verb::Get && next.method == Method::List);
    assert!(next
        .url
        .starts_with("https://people.googleapis.com/v1/people/me/connections?"));
}

#[test]
fn the_pinned_parameters_hash_is_stable_and_names_what_is_pinned() {
    let h = list_params_hash();
    assert_eq!(h, list_params_hash());
    assert_eq!(h.len(), 32);
    assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
    // Pinned letter for letter. The hash is stored beside every user's sync token, so changing this
    // text (an order, a separator, a parameter, the mask) makes every stored hash mismatch and
    // forces a full sync, with its first-page quota, for everyone.
    assert_eq!(
        pinned_params(),
        "pageSize=1000;requestSyncToken=true;personFields=names,nicknames,birthdays,events,emailAddresses,phoneNumbers,addresses,organizations,urls,imClients,relations,biographies,userDefined,memberships,clientData,metadata;sources=READ_SOURCE_TYPE_CONTACT"
    );
    assert_eq!(
        h,
        hex::encode(Sha256::digest(pinned_params().as_bytes()))[..32]
    );
}

#[test]
fn a_batch_get_names_each_contact_and_stops_at_the_limit() {
    let names: Vec<String> = (1..=BATCH_GET_MAX)
        .map(|i| format!("people/c{i}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let p = batch_get(&refs).unwrap();
    let pairs = query(&p);
    assert_eq!(q(&pairs, "resourceNames").len(), BATCH_GET_MAX);
    assert!(p
        .url
        .starts_with("https://people.googleapis.com/v1/people:batchGet?"));
    let mut over = refs.clone();
    over.push("people/c9999");
    assert_eq!(batch_get(&over).unwrap_err(), PlanError::TooMany);
}

#[test]
fn the_account_address_call_sends_no_sources() {
    let p = me_address();
    let pairs = query(&p);
    assert_eq!(q(&pairs, "personFields"), ["emailAddresses"]);
    assert!(
        q(&pairs, "sources").is_empty(),
        "the profile must apply: {pairs:?}"
    );
    assert!(p
        .url
        .starts_with("https://people.googleapis.com/v1/people/me?"));
    assert!(p.verb == Verb::Get && p.body.is_none());
}

#[test]
fn a_create_is_a_post_that_is_never_retried_and_carries_the_pm_id() {
    let body = crate::people_google::map::person_for_create(
        &Card {
            display_name: "Sam Example".into(),
            given_name: "Sam".into(),
            ..Default::default()
        },
        false,
        &[],
        "0123456789abcdef0123456789abcdef",
    );
    let p = create_contact(body);
    assert!(p.verb == Verb::Post && p.method == Method::Create);
    assert!(p
        .url
        .starts_with("https://people.googleapis.com/v1/people:createContact?"));
    assert!(p.body.as_ref().unwrap()["clientData"][0]["key"] == "pm.uid");
    assert!(!crate::people_google::classify::should_retry(
        p.method,
        crate::people_google::classify::Verdict::ServerError,
        1
    ));
}

#[test]
fn the_plans_debug_names_no_value() {
    let p = create_contact(json!({ "names": [{ "givenName": "Zebediah" }] }));
    let text = format!("{p:?}");
    assert!(!text.contains("Zebediah") && !text.contains("googleapis"));
}

// --- contact groups ------------------------------------------------------------------------------------------

#[test]
fn deleting_a_group_never_deletes_its_contacts() {
    let p = delete_group("contactGroups/aa11").unwrap();
    assert_eq!(q(&query(&p), "deleteContacts"), ["false"]);
    assert!(p.verb == Verb::Delete);
}

#[test]
fn group_creation_and_rename_are_planned_as_google_documents_them() {
    let c = create_group("Climbing");
    assert!(c.verb == Verb::Post && c.method == Method::Create);
    assert!(c.url == "https://people.googleapis.com/v1/contactGroups");
    assert_eq!(c.body.as_ref().unwrap()["contactGroup"]["name"], "Climbing");
    let r = rename_group("contactGroups/aa11", "grp-etag", "Bouldering").unwrap();
    assert!(r.verb == Verb::Put && r.method == Method::Update);
    assert_eq!(r.url, "https://people.googleapis.com/v1/contactGroups/aa11");
    let body = r.body.as_ref().unwrap();
    assert_eq!(body["contactGroup"]["etag"], "grp-etag");
    assert_eq!(body["updateGroupFields"], "name");
    let l = list_groups(Some("tok"), None);
    assert_eq!(q(&query(&l), "syncToken"), ["tok"]);
    assert!(l.method == Method::List);
}

// --- updateContact ------------------------------------------------------------------------------------------------

#[test]
fn an_email_edit_updates_its_google_item_in_place() {
    let f = fresh();
    let plan = send_plan(&f, |pm, _| {
        let at = row(pm, "email", 1);
        pm.details[at].value = "sam.home2@example.com".into();
    })
    .expect("a change to send");
    let body = plan.body.as_ref().unwrap();
    // The etag guards the write, and only the changed array is named.
    assert_eq!(body["metadata"]["sources"][0]["type"], "CONTACT");
    assert_eq!(body["metadata"]["sources"][0]["etag"], "etag-7");
    assert_eq!(q(&query(&plan), "updatePersonFields"), ["emailAddresses"]);
    assert!(plan.verb == Verb::Patch && plan.method == Method::Update);
    let emails = body["emailAddresses"].as_array().unwrap();
    assert_eq!(emails.len(), 2, "edited in place, not removed and added");
    // The first is untouched, primary stated in its writable form, display name kept.
    assert_eq!(emails[0]["value"], "sam@example.com");
    assert_eq!(emails[0]["displayName"], "Sam at work");
    assert!(emails[0]["metadata"] == json!({ "sourcePrimary": true }));
    // The second has its new value and everything else it had.
    assert_eq!(emails[1]["value"], "sam.home2@example.com");
    assert_eq!(emails[1]["type"], "home");
    assert_eq!(emails[1]["displayName"], "Sam at home");
    // Output-only keys are not sent back.
    assert!(emails[0].get("formattedType").is_none());
    assert!(body.get("names").is_none() && body.get("phoneNumbers").is_none());
}

#[test]
fn an_update_keeps_attributes_pm_doesnt_model() {
    let f = fresh();
    let plan = send_plan(&f, |pm, _| {
        pm.given_name = "Samuel".into();
        let p = row(pm, "phone", 0);
        pm.details[p].value = "+44 7700 900199".into();
        let w = row(pm, "work", 0);
        pm.details[w].parts = Some(json!({ "title": "Lead", "department": "Tools" }));
        let a = row(pm, "address", 0);
        pm.details[a].parts = Some(json!({
            "street": "1 Example Road", "city": "Sampleton", "postal_code": "EX1 2AB",
            "po_box": "PO 7", "country": "Elsewhere"
        }));
        pm.notes = "Met at the fair. Likes tea.".into();
    })
    .unwrap();
    let body = plan.body.as_ref().unwrap();
    // Only what changed is named, in a fixed order.
    assert_eq!(
        q(&query(&plan), "updatePersonFields"),
        ["names,phoneNumbers,addresses,organizations,biographies"]
    );
    // The name: PM's part written, Google's phonetic name kept, the free-form twin gone.
    let n = &body["names"][0];
    assert_eq!(n["givenName"], "Samuel");
    assert_eq!(n["familyName"], "Example");
    assert_eq!(n["phoneticGivenName"], "Sahm");
    assert!(n.get("unstructuredName").is_none() && n.get("displayName").is_none());
    assert_eq!(
        body["names"].as_array().unwrap().len(),
        1,
        "a singleton never carries a second value"
    );
    // The phone: new value, Google's canonical form dropped (it is output only).
    assert!(body["phoneNumbers"][0] == json!({ "value": "+44 7700 900199", "type": "mobile" }));
    // The organization: PM's title, Google's cost centre and phonetic name still there.
    let o = &body["organizations"][0];
    assert_eq!(o["title"], "Lead");
    assert_eq!(o["costCenter"], "CC-9");
    assert_eq!(o["phoneticName"], "Egg-zample");
    // The address: PM's country, Google's country code and PO box kept, the stale text gone.
    let a = &body["addresses"][0];
    assert_eq!(a["country"], "Elsewhere");
    assert_eq!(a["countryCode"], "XX");
    assert_eq!(a["poBox"], "PO 7");
    assert!(a.get("formattedValue").is_none());
    assert_eq!(
        body["biographies"][0]["value"],
        "Met at the fair. Likes tea."
    );
    assert_eq!(body["biographies"].as_array().unwrap().len(), 1);
    // Fields PM changed nothing in are not listed or sent.
    for untouched in [
        "emailAddresses",
        "urls",
        "imClients",
        "birthdays",
        "memberships",
        "events",
    ] {
        assert!(body.get(untouched).is_none(), "{untouched}");
    }
}

#[test]
fn an_added_item_is_appended_and_a_removed_one_is_taken_out_of_googles_array() {
    let f = fresh();
    let plan = send_plan(&f, |pm, src| {
        // PM drops the home email and adds a new web link and a nickname.
        let at = row(pm, "email", 1);
        pm.details.remove(at);
        src.remove(at);
        pm.details.push(CardDetail {
            kind: "web".into(),
            label: "Instagram".into(),
            value: "https://example.com/sam-ig".into(),
            ..Default::default()
        });
        pm.details.push(CardDetail {
            kind: "nickname".into(),
            value: "Sammy".into(),
            ..Default::default()
        });
        src.extend([String::new(), String::new()]);
    })
    .unwrap();
    let body = plan.body.as_ref().unwrap();
    assert_eq!(
        q(&query(&plan), "updatePersonFields"),
        ["nicknames,emailAddresses,urls"]
    );
    let emails = body["emailAddresses"].as_array().unwrap();
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0]["value"], "sam@example.com");
    // The new link joins Google's own url, and the chat handle (a different array) is left alone.
    let urls = body["urls"].as_array().unwrap();
    assert_eq!(urls.len(), 2);
    assert_eq!(urls[0]["value"], "https://example.com/sam");
    assert!(urls[1] == json!({ "value": "https://example.com/sam-ig", "type": "Instagram" }));
    assert!(body.get("imClients").is_none());
    assert!(body["nicknames"] == json!([{ "value": "Sammy", "type": "DEFAULT" }]));
}

#[test]
fn a_chat_handle_edit_goes_to_its_own_array() {
    let f = fresh();
    let plan = send_plan(&f, |pm, _| {
        let at = pm
            .details
            .iter()
            .position(|d| d.value == "sam#0001")
            .unwrap();
        pm.details[at].value = "sam#0002".into();
    })
    .unwrap();
    let body = plan.body.as_ref().unwrap();
    assert_eq!(q(&query(&plan), "updatePersonFields"), ["imClients"]);
    assert!(body["imClients"] == json!([{ "username": "sam#0002", "protocol": "Discord" }]));
}

#[test]
fn nothing_to_change_plans_nothing() {
    let f = fresh();
    // PM "edits" a value to what Google already holds.
    assert!(send_plan(&f, |_, _| {}).is_none());
    let none = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &f,
        send: &[Send::EditItem {
            row: 0,
            kind: "email".into(),
            google_key: "sam@example.com".into(),
            attrs: vec!["value".into()],
            to: CardDetail {
                kind: "email".into(),
                value: "sam@example.com".into(),
                ..Default::default()
            },
            primary: false,
        }],
        groups: &groups(),
    })
    .unwrap();
    assert!(none.is_none());
    // An item the fresh copy no longer has is never invented, and never read as "already done":
    // the caller must read the contact again and merge again, or a dropped edit would look like
    // nothing to change and its dirty mark would be cleared.
    let gone = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &f,
        send: &[Send::RemoveItem {
            kind: "email".into(),
            google_key: "nobody@example.com".into(),
        }],
        groups: &groups(),
    });
    assert_eq!(gone.err(), Some(PlanError::ItemMissing { at: 0 }));
    let lost_edit = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &f,
        send: &[Send::EditItem {
            row: 0,
            kind: "email".into(),
            google_key: "nobody@example.com".into(),
            attrs: vec!["value".into()],
            to: CardDetail {
                kind: "email".into(),
                value: "new@example.com".into(),
                ..Default::default()
            },
            primary: false,
        }],
        groups: &groups(),
    });
    assert_eq!(lost_edit.err(), Some(PlanError::ItemMissing { at: 0 }));
}

#[test]
fn a_school_field_edit_becomes_an_organizations_title_in_place() {
    let mut f = fresh();
    f["organizations"] = json!([{ "name": "Example College", "title": "Maths", "type": "school",
        "costCenter": "CC-1" }]);
    let plan = send_plan(&f, |pm, _| {
        let at = row(pm, "school", 0);
        assert_eq!(pm.details[at].parts.as_ref().unwrap()["field"], "Maths");
        pm.details[at].parts = Some(json!({ "field": "Physics" }));
    })
    .expect("a change to send");
    assert_eq!(q(&query(&plan), "updatePersonFields"), ["organizations"]);
    let o = &plan.body.as_ref().unwrap()["organizations"][0];
    assert_eq!(o["title"], "Physics");
    assert_eq!(o["type"], "school");
    assert_eq!(o["costCenter"], "CC-1", "what PM does not model stays");
}

#[test]
fn clearing_every_name_part_clears_the_name_and_keeps_a_free_form_one() {
    // Every part cleared: the name goes, it is not sent as an empty object.
    let mut f = fresh();
    f["names"] =
        json!([{ "givenName": "Sam", "familyName": "Example", "displayName": "Sam Example" }]);
    let plan = send_plan(&f, |pm, _| {
        pm.given_name = String::new();
        pm.family_name = String::new();
    })
    .expect("a change to send");
    assert!(plan.body.as_ref().unwrap()["names"] == json!([]));

    // A name Google holds only as free-form text keeps it when no part is written.
    let mut only_free = fresh();
    only_free["names"] =
        json!([{ "unstructuredName": "Sam Example", "displayName": "Sam Example" }]);
    let upd = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &only_free,
        send: &[Send::Set(Single::GivenName, None)],
        groups: &groups(),
    })
    .unwrap();
    // Nothing changed in the parts and the name is as Google sent it apart from its output-only keys.
    let body = upd.expect("the output-only keys differ").body.unwrap();
    assert_eq!(body["names"][0]["unstructuredName"], "Sam Example");
    assert!(body["names"][0].get("displayName").is_none());
}

#[test]
fn a_note_longer_than_pm_keeps_is_not_overwritten_with_its_cut_copy() {
    let mut f = fresh();
    f["biographies"] = json!([{ "value": "x".repeat(crate::people::NOTES_MAX + 1),
        "contentType": "TEXT_PLAIN" }]);
    let err = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &f,
        send: &[
            Send::Set(Single::Starred, Some("1".into())),
            Send::Set(Single::Notes, Some("short".into())),
        ],
        groups: &groups(),
    })
    .unwrap_err();
    // The error names which send is stuck, so the caller can drop it and plan the rest.
    assert_eq!(err, PlanError::NotesTooLong { at: 1 });
}

#[test]
fn a_birthday_and_a_note_are_singletons_and_clear_with_an_empty_array() {
    let f = fresh();
    let plan = send_plan(&f, |pm, _| {
        pm.born_on = Some("1990-04-15".into());
        pm.notes = String::new();
    })
    .unwrap();
    let body = plan.body.as_ref().unwrap();
    assert_eq!(
        q(&query(&plan), "updatePersonFields"),
        ["birthdays,biographies"]
    );
    assert!(body["birthdays"] == json!([{ "date": { "year": 1990, "month": 4, "day": 15 } }]));
    assert!(body["biographies"] == json!([]));
}

#[test]
fn the_primary_choice_is_written_as_source_primary_on_one_item() {
    let f = fresh();
    let plan = send_plan(&f, |pm, src| {
        let a = row(pm, "email", 0);
        pm.details.swap(a, a + 1);
        src.swap(a, a + 1);
    })
    .unwrap();
    let body = plan.body.as_ref().unwrap();
    let emails = body["emailAddresses"].as_array().unwrap();
    // Google's order is kept; the flag moved to the home address.
    assert_eq!(emails[0]["value"], "sam@example.com");
    assert!(emails[0].get("metadata").is_none());
    assert!(emails[1]["metadata"] == json!({ "sourcePrimary": true }));
    assert_eq!(emails[1]["displayName"], "Sam at home");
}

#[test]
fn a_missing_etag_is_refused_and_says_nothing_of_the_contact() {
    let err = update_contact(&UpdateInput {
        resource: "people/c1",
        fresh: &json!({ "metadata": { "sources": [{ "type": "PROFILE", "etag": "p" }] } }),
        send: &[Send::Set(Single::Notes, Some("x".into()))],
        groups: &groups(),
    })
    .unwrap_err();
    assert_eq!(err, PlanError::MissingEtag);
    assert!(!err.to_string().contains("people/c1"));
}

// --- memberships -----------------------------------------------------------------------------------------------

#[test]
fn a_membership_write_keeps_my_contacts_and_the_system_groups_pm_doesnt_own() {
    let f = fresh();
    let plan = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &f,
        send: &[
            Send::Set(Single::Starred, Some("1".into())),
            Send::AddLabel("Climbing".into()),
        ],
        groups: &groups(),
    })
    .unwrap()
    .unwrap();
    assert_eq!(q(&query(&plan), "updatePersonFields"), ["memberships"]);
    let names: Vec<&str> = plan.body.as_ref().unwrap()["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            m["contactGroupMembership"]["contactGroupResourceName"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        names,
        [
            "contactGroups/myContacts",
            "contactGroups/friends",
            "contactGroups/6a2b3c4d5e",
            "contactGroups/starred",
            "contactGroups/aa11"
        ]
    );

    // Removing a label and the star leaves the rest, including a system group.
    let mut with_star = f.clone();
    with_star["memberships"].as_array_mut().unwrap().push(
        json!({ "contactGroupMembership": { "contactGroupResourceName": "contactGroups/starred" } }),
    );
    let plan = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &with_star,
        send: &[
            Send::Set(Single::Starred, None),
            Send::RemoveLabel("Friends Club".into()),
        ],
        groups: &groups(),
    })
    .unwrap()
    .unwrap();
    let names: Vec<&str> = plan.body.as_ref().unwrap()["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            m["contactGroupMembership"]["contactGroupResourceName"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(names, ["contactGroups/myContacts", "contactGroups/friends"]);
}

#[test]
fn an_update_never_leaves_a_contact_with_no_group() {
    let f = json!({
        "metadata": { "sources": [{ "type": "CONTACT", "etag": "e" }] },
        "memberships": [{ "contactGroupMembership": { "contactGroupResourceName": "contactGroups/6a2b3c4d5e" } }]
    });
    let plan = update_contact(&UpdateInput {
        resource: "people/c1",
        fresh: &f,
        send: &[Send::RemoveLabel("Friends Club".into())],
        groups: &groups(),
    })
    .unwrap()
    .unwrap();
    assert!(
        plan.body.as_ref().unwrap()["memberships"]
            == json!([{ "contactGroupMembership": { "contactGroupResourceName": "contactGroups/myContacts" } }])
    );
}

#[test]
fn a_label_with_no_group_yet_is_refused_by_name_of_nothing() {
    let err = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &fresh(),
        send: &[Send::AddLabel("Brand new label".into())],
        groups: &groups(),
    })
    .unwrap_err();
    assert_eq!(err, PlanError::UnknownLabel);
    assert!(!err.to_string().contains("Brand"));
    // Removing a label PM never mapped has nothing to remove.
    let none = update_contact(&UpdateInput {
        resource: "people/c1234567890",
        fresh: &fresh(),
        send: &[Send::RemoveLabel("Brand new label".into())],
        groups: &groups(),
    })
    .unwrap();
    assert!(none.is_none());
}
