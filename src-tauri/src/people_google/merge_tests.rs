// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The three-way merge, rule by rule (spec 7.3, 7.4, 12.2). Every contact here is invented. Cards
//! hold contact values and have no `Debug`, so the checks compare with `assert!` and name the case
//! in the message.

use super::*;
use serde_json::json;

const NOW: &str = "2026-11-27T10:00:00.000Z";

fn d(kind: &str, label: &str, value: &str) -> CardDetail {
    CardDetail {
        kind: kind.into(),
        label: label.into(),
        value: value.into(),
        ..Default::default()
    }
}

fn job(name: &str, title: &str, dept: &str) -> CardDetail {
    let mut parts = serde_json::Map::new();
    if !title.is_empty() {
        parts.insert("title".into(), json!(title));
    }
    if !dept.is_empty() {
        parts.insert("department".into(), json!(dept));
    }
    CardDetail {
        parts: (!parts.is_empty()).then_some(Value::Object(parts)),
        ..d("work", "", name)
    }
}

fn card(details: &[CardDetail]) -> Card {
    Card {
        display_name: "Sam Example".into(),
        given_name: "Sam".into(),
        family_name: "Example".into(),
        details: details.to_vec(),
        ..Default::default()
    }
}

fn run(base: &Card, pm: &Card, src: &[&str], google: &Card, open: &[Conflict]) -> Outcome {
    let src: Vec<String> = src.iter().map(|s| s.to_string()).collect();
    merge(
        Some(&Side {
            card: base,
            starred: false,
            src_keys: &[],
        }),
        &Side {
            card: pm,
            starred: false,
            src_keys: &src,
        },
        &Side {
            card: google,
            starred: false,
            src_keys: &[],
        },
        open,
        NOW,
    )
}

fn quiet(o: &Outcome, case: &str) {
    assert!(o.apply.is_empty(), "{case}: nothing to apply");
    assert!(o.send.is_empty(), "{case}: nothing to send");
    assert!(o.conflicts.is_empty(), "{case}: no conflict");
}

// --- single values --------------------------------------------------------------------------------------

#[test]
fn a_single_value_follows_whoever_changed_it() {
    let base = card(&[]);
    let mut changed = base.clone();
    changed.given_name = "Samuel".into();
    changed.notes = "Met at the fair".into();
    changed.born_on = Some("1990-04-15".into());

    // Google changed, PM didn't: apply.
    let o = run(&base, &base, &[], &changed, &[]);
    assert!(o.send.is_empty() && o.conflicts.is_empty());
    let words: Vec<&str> = o
        .apply
        .iter()
        .map(|a| match a {
            Apply::Set(f, _) => f.word(),
            _ => "other",
        })
        .collect();
    assert_eq!(words, ["given_name", "birthday", "notes"]);

    // PM changed, Google didn't: send, and say which fields.
    let o = run(&base, &changed, &[], &base, &[]);
    assert!(o.apply.is_empty() && o.conflicts.is_empty());
    assert_eq!(o.changed_fields(), ["birthday", "given_name", "notes"]);

    // Both changed to the same thing: nothing.
    quiet(&run(&base, &changed, &[], &changed, &[]), "agree");
}

#[test]
fn both_changing_a_value_differently_is_a_conflict() {
    let base = card(&[]);
    let (mut pm, mut google) = (base.clone(), base.clone());
    pm.given_name = "Samuel".into();
    google.given_name = "Sammy".into();
    let o = run(&base, &pm, &[], &google, &[]);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert_eq!(o.conflicts.len(), 1);
    let c = &o.conflicts[0];
    assert_eq!(c.field, "given_name");
    assert!(c.item_key.is_none() && c.attribute.is_none());
    assert!(c.pm == json!("Samuel") && c.google == json!("Sammy"));
    assert_eq!(c.seen_at, NOW);
}

#[test]
fn the_star_is_a_single_value_too() {
    let base = card(&[]);
    let side = |starred| Side {
        card: &base,
        starred,
        src_keys: &[],
    };
    let o = merge(Some(&side(false)), &side(true), &side(false), &[], NOW);
    assert!(matches!(
        o.send.as_slice(),
        [Send::Set(Single::Starred, Some(_))]
    ));
    let o = merge(Some(&side(false)), &side(false), &side(true), &[], NOW);
    assert!(matches!(
        o.apply.as_slice(),
        [Apply::Set(Single::Starred, Some(_))]
    ));
    assert!(o.snapshot_starred);
    // PM unstarred what Google still has starred.
    let o = merge(Some(&side(true)), &side(false), &side(true), &[], NOW);
    assert!(matches!(
        o.send.as_slice(),
        [Send::Set(Single::Starred, None)]
    ));
}

// --- lists: pairing -----------------------------------------------------------------------------------------

#[test]
fn items_pair_as_a_multiset_in_order_of_appearance() {
    let a = d("email", "", "twin@example.com");
    // Two identical items; Google drops one: the second of PM's goes.
    let base = card(&[a.clone(), a.clone()]);
    let google = card(std::slice::from_ref(&a));
    let o = run(&base, &base, &[], &google, &[]);
    assert!(o.send.is_empty() && o.conflicts.is_empty());
    assert!(matches!(o.apply.as_slice(), [Apply::RemoveItem { row: 1 }]));

    // PM adds a duplicate of what both already hold: sent as an addition, once.
    let pm = card(&[a.clone(), a.clone()]);
    let base = card(std::slice::from_ref(&a));
    let o = run(&base, &pm, &[], &base, &[]);
    assert!(o.apply.is_empty() && o.conflicts.is_empty());
    assert!(matches!(o.send.as_slice(), [Send::AddItem { row: 1, .. }]));
}

#[test]
fn an_item_added_on_one_side_is_added_and_one_removed_is_removed() {
    let a = d("email", "work", "a@example.com");
    let b = d("phone", "", "+44 7700 900104");
    let base = card(std::slice::from_ref(&a));

    // Google added a phone.
    let o = run(&base, &base, &[], &card(&[a.clone(), b.clone()]), &[]);
    assert!(o.send.is_empty() && o.conflicts.is_empty());
    assert!(
        matches!(o.apply.as_slice(), [Apply::AddItem { detail, .. }] if detail.value == b.value)
    );

    // PM added it.
    let o = run(&base, &card(&[a.clone(), b.clone()]), &[], &base, &[]);
    assert!(o.apply.is_empty() && o.conflicts.is_empty());
    assert!(matches!(o.send.as_slice(), [Send::AddItem { detail, .. }] if detail.value == b.value));
    assert_eq!(o.changed_fields(), ["phone"]);

    // Both added the same phone: agreed, nothing to do.
    quiet(
        &run(
            &base,
            &card(&[a.clone(), b.clone()]),
            &[],
            &card(&[a.clone(), b.clone()]),
            &[],
        ),
        "same addition",
    );

    // Google removed the email PM left alone: removed in PM.
    let o = run(&base, &base, &[], &card(&[]), &[]);
    assert!(matches!(o.apply.as_slice(), [Apply::RemoveItem { row: 0 }]));
    // PM removed what Google left alone: removed in Google, by the key of Google's item.
    let o = run(&base, &card(&[]), &[], &base, &[]);
    assert!(
        matches!(o.send.as_slice(), [Send::RemoveItem { kind, google_key }]
        if kind == "email" && google_key == "a@example.com")
    );
    // Both removed it: nothing.
    quiet(
        &run(&base, &card(&[]), &[], &card(&[]), &[]),
        "both removed",
    );
}

#[test]
fn position_matters_only_as_primary_or_not() {
    let (a, b, c) = (
        d("email", "", "a@example.com"),
        d("email", "", "b@example.com"),
        d("email", "", "c@example.com"),
    );
    let base = card(&[a.clone(), b.clone(), c.clone()]);
    // Swapping two non-primary items is no change at all.
    quiet(
        &run(
            &base,
            &card(&[a.clone(), c.clone(), b.clone()]),
            &[],
            &base,
            &[],
        ),
        "swap of the non-primary",
    );
    // Google reordering what PM left alone is no change either.
    quiet(
        &run(
            &base,
            &base,
            &[],
            &card(&[a.clone(), c.clone(), b.clone()]),
            &[],
        ),
        "google reorder",
    );
    // Making another item the primary one is: exactly the two primary flags move.
    let o = run(
        &base,
        &card(&[b.clone(), a.clone(), c.clone()]),
        &[],
        &base,
        &[],
    );
    assert!(o.apply.is_empty() && o.conflicts.is_empty());
    assert_eq!(o.send.len(), 2);
    for s in &o.send {
        match s {
            Send::EditItem {
                attrs,
                google_key,
                primary,
                ..
            } => {
                assert_eq!(attrs.as_slice(), ["primary"]);
                assert_eq!(*primary, google_key == "b@example.com");
            }
            _ => panic!("only primary edits"),
        }
    }
}

#[test]
fn removing_the_primary_item_does_not_look_like_an_edit_of_the_next() {
    let (a, b) = (
        d("email", "", "a@example.com"),
        d("email", "", "b@example.com"),
    );
    let base = card(&[a.clone(), b.clone()]);
    // PM removes the first (b becomes the only one): only the removal goes out.
    let o = run(&base, &card(std::slice::from_ref(&b)), &[], &base, &[]);
    assert!(o.apply.is_empty() && o.conflicts.is_empty());
    assert!(
        matches!(o.send.as_slice(), [Send::RemoveItem { google_key, .. }] if google_key == "a@example.com")
    );
}

// --- lists: attributes ---------------------------------------------------------------------------------------

#[test]
fn a_paired_item_is_compared_attribute_by_attribute() {
    let base = card(&[job("Example Works", "Engineer", "Tools")]);
    // PM changes the title, Google the department: both go through, no conflict.
    let pm = card(&[job("Example Works", "Lead", "Tools")]);
    let google = card(&[job("Example Works", "Engineer", "Build")]);
    let o = run(&base, &pm, &[], &google, &[]);
    assert!(o.conflicts.is_empty());
    match o.send.as_slice() {
        [Send::EditItem {
            attrs,
            kind,
            google_key,
            ..
        }] => {
            assert_eq!(attrs.as_slice(), ["parts.title"]);
            assert_eq!(kind, "work");
            assert_eq!(google_key, "example works");
        }
        _ => panic!("one send"),
    }
    match o.apply.as_slice() {
        [Apply::EditItem { attrs, to, row, .. }] => {
            assert_eq!(attrs.as_slice(), ["parts.department"]);
            assert_eq!(*row, 0);
            // The applied row keeps PM's own title and takes Google's department.
            let p = to.parts.as_ref().unwrap();
            assert_eq!(
                (p["title"].as_str(), p["department"].as_str()),
                (Some("Lead"), Some("Build"))
            );
        }
        _ => panic!("one apply"),
    }
}

#[test]
fn the_same_attribute_changed_two_ways_is_a_conflict_on_that_attribute() {
    let base = card(&[d("email", "work", "a@example.com")]);
    let pm = card(&[d("email", "home", "a@example.com")]);
    let google = card(&[d("email", "other", "a@example.com")]);
    let o = run(&base, &pm, &[], &google, &[]);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert_eq!(o.conflicts.len(), 1);
    let c = &o.conflicts[0];
    assert_eq!(c.field, "email");
    assert_eq!(c.item_key.as_deref(), Some("a@example.com"));
    assert_eq!(c.attribute.as_deref(), Some("label"));
    assert!(c.pm == json!("home") && c.google == json!("other"));
    // The label compares folded: a retyped case is no change.
    let same = card(&[d("email", "WORK", "a@example.com")]);
    quiet(&run(&base, &same, &[], &base, &[]), "case of a label");
}

#[test]
fn a_job_compares_its_dates_and_current_flag() {
    let mut a = job("Example Works", "Engineer", "");
    a.from_date = Some("2019-03".into());
    let base = card(&[a.clone()]);
    let mut pm_job = a.clone();
    pm_job.is_current = true;
    let mut g_job = a.clone();
    g_job.to_date = Some("2022".into());
    let o = run(&base, &card(&[pm_job]), &[], &card(&[g_job]), &[]);
    assert!(o.conflicts.is_empty());
    assert!(
        matches!(o.send.as_slice(), [Send::EditItem { attrs, .. }] if attrs.as_slice() == ["current"])
    );
    assert!(
        matches!(o.apply.as_slice(), [Apply::EditItem { attrs, .. }] if attrs.as_slice() == ["to"])
    );
    // PM-only dates on an email are not Google's: never compared.
    let mut e = d("email", "", "a@example.com");
    let base = card(&[e.clone()]);
    e.from_date = Some("2020".into());
    quiet(
        &run(&base, &card(&[e]), &[], &base, &[]),
        "dates on an email",
    );
}

#[test]
fn an_address_compares_by_part_and_a_flat_street_is_the_street_part() {
    let full = |country: &str, region: &str| CardDetail {
        parts: Some(
            json!({ "street": "1 Example Road", "city": "Sampleton", "country": country, "region": region }),
        ),
        ..d("address", "home", "1 Example Road\nSampleton")
    };
    // Country and region are not in the matching key, so edits keep the pairing.
    let base = card(&[full("Exland", "")]);
    let o = run(
        &base,
        &card(&[full("Exland", "Exshire")]),
        &[],
        &card(&[full("Elsewhere", "")]),
        &[],
    );
    assert!(o.conflicts.is_empty());
    assert!(
        matches!(o.send.as_slice(), [Send::EditItem { attrs, .. }] if attrs.as_slice() == ["parts.region"])
    );
    assert!(
        matches!(o.apply.as_slice(), [Apply::EditItem { attrs, to, .. }]
        if attrs.as_slice() == ["parts.country"] && to.parts.as_ref().unwrap()["country"] == "Elsewhere"
            // The applied row keeps PM's own region.
            && to.parts.as_ref().unwrap()["region"] == "Exshire")
    );
}

// --- both replaced, removed vs edited, src_key -----------------------------------------------------------------

#[test]
fn both_sides_replacing_an_item_keeps_both_new_values() {
    let base = card(&[d("email", "", "old@example.com")]);
    let pm = card(&[d("email", "", "mine@example.com")]);
    let google = card(&[d("email", "", "theirs@example.com")]);
    // PM's row has no src_key: it is a removal plus an addition on each side.
    let o = run(&base, &pm, &[""], &google, &[]);
    assert!(o.conflicts.is_empty());
    assert!(
        matches!(o.apply.as_slice(), [Apply::AddItem { detail, .. }] if detail.value == "theirs@example.com")
    );
    assert!(o
        .send
        .iter()
        .any(|s| matches!(s, Send::AddItem { detail, .. } if detail.value == "mine@example.com")));
    assert!(
        !o.send.iter().any(
            |s| matches!(s, Send::RemoveItem { google_key, .. } if google_key == "old@example.com")
        ),
        "Google already lost the old one, so there is nothing to remove there"
    );
}

#[test]
fn removed_on_one_side_and_edited_on_the_other_is_a_conflict() {
    let base = card(&[d("email", "work", "a@example.com")]);

    // PM removed it, Google edited its label.
    let google = card(&[d("email", "home", "a@example.com")]);
    let o = run(&base, &card(&[]), &[], &google, &[]);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert_eq!(o.conflicts.len(), 1);
    let c = &o.conflicts[0];
    assert_eq!(
        (
            c.field.as_str(),
            c.item_key.as_deref(),
            c.attribute.as_deref()
        ),
        ("email", Some("a@example.com"), None)
    );
    assert!(c.pm.is_null() && !c.google.is_null());

    // Google removed it, PM edited its label: the row is held.
    let pm = card(&[d("email", "home", "a@example.com")]);
    let o = run(&base, &pm, &[], &card(&[]), &[]);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert_eq!(o.conflicts.len(), 1);
    assert!(!o.conflicts[0].pm.is_null() && o.conflicts[0].google.is_null());
    assert_eq!(o.frozen_rows, [0]);
}

#[test]
fn an_edited_value_pairs_with_its_google_item_through_src_key() {
    let base = card(&[d("email", "work", "a@example.com")]);
    // PM edited a@ to b@ (its row still remembers a@); Google didn't touch it: sent as an edit
    // of Google's own item, not as a removal and an addition.
    let pm = card(&[d("email", "work", "b@example.com")]);
    let o = run(&base, &pm, &["a@example.com"], &base, &[]);
    assert!(o.apply.is_empty() && o.conflicts.is_empty());
    match o.send.as_slice() {
        [Send::EditItem {
            attrs,
            google_key,
            to,
            row,
            ..
        }] => {
            assert_eq!(attrs.as_slice(), ["value"]);
            assert_eq!(google_key, "a@example.com");
            assert_eq!(to.value, "b@example.com");
            assert_eq!(*row, 0);
        }
        _ => panic!("one edit in place"),
    }
    // Without the src_key the same edit is a removal plus an addition.
    let o = run(&base, &pm, &[""], &base, &[]);
    assert_eq!(o.send.len(), 2);

    // Google edited a@ to the same b@: agreed.
    let google = card(&[d("email", "work", "b@example.com")]);
    quiet(
        &run(&base, &pm, &["a@example.com"], &google, &[]),
        "same edit",
    );
}

#[test]
fn pm_editing_a_to_b_while_google_edits_a_to_c_offers_all_three_answers() {
    let base = card(&[d("email", "work", "a@example.com")]);
    let pm = card(&[d("email", "work", "b@example.com")]);
    let google = card(&[d("email", "work", "c@example.com")]);
    let o = run(&base, &pm, &["a@example.com"], &google, &[]);
    assert!(
        o.apply.is_empty() && o.send.is_empty(),
        "frozen: neither side moves"
    );
    assert_eq!(o.conflicts.len(), 1);
    let c = &o.conflicts[0];
    assert_eq!(c.item_key.as_deref(), Some("a@example.com"));
    assert!(c.attribute.is_none());
    assert_eq!(c.pm["value"], "b@example.com");
    assert_eq!(c.google["value"], "c@example.com");
    // PM's row keeps its src_key until answered.
    assert_eq!(o.frozen_rows, [0]);
    // The snapshot is a pure copy of Google's.
    assert!(o.snapshot == google);
}

// --- labels --------------------------------------------------------------------------------------------------

#[test]
fn labels_are_a_set_by_folded_name() {
    let with = |names: &[&str]| {
        let mut c = card(&[]);
        c.labels = names.iter().map(|s| s.to_string()).collect();
        c
    };
    let base = with(&["Friends"]);
    // PM adds one, Google drops another.
    let o = run(&base, &with(&["Friends", "Climbing"]), &[], &with(&[]), &[]);
    assert!(o.conflicts.is_empty());
    assert!(matches!(o.send.as_slice(), [Send::AddLabel(n)] if n == "Climbing"));
    assert!(matches!(o.apply.as_slice(), [Apply::RemoveLabel(n)] if n == "Friends"));
    assert_eq!(o.changed_fields(), ["label"]);
    // A retyped case is the same label.
    quiet(
        &run(&base, &with(&["friends"]), &[], &base, &[]),
        "case of a label name",
    );
    // Google adds one, PM removes one.
    let o = run(&base, &with(&[]), &[], &with(&["Friends", "Family"]), &[]);
    assert!(matches!(o.send.as_slice(), [Send::RemoveLabel(n)] if n == "Friends"));
    assert!(matches!(o.apply.as_slice(), [Apply::AddLabel(n)] if n == "Family"));
}

#[test]
fn a_kind_pm_does_not_send_is_never_merged() {
    // Where they are now is PM's own: a change to it is neither sent nor asked about.
    let base = card(&[]);
    let pm = card(&[d("location", "", "Lisbon")]);
    quiet(&run(&base, &pm, &[], &base, &[]), "a PM-only detail");
}

// --- open conflicts freeze their field --------------------------------------------------------------------------

#[test]
fn an_open_conflict_survives_a_later_write_and_pull_unchanged() {
    // The conflict: both changed the given name.
    let base = card(&[]);
    let (mut pm, mut google) = (base.clone(), base.clone());
    pm.given_name = "Samuel".into();
    google.given_name = "Sammy".into();
    let first = run(&base, &pm, &[], &google, &[]);
    assert_eq!(first.conflicts.len(), 1);
    let stored = first.conflicts.clone();

    // A later write: PM adds a phone and sends it. The reply makes Google's copy the new base
    // (given name Sammy, plus the phone). Then a pull finds Google changed the name again.
    let phone = d("phone", "", "+44 7700 900105");
    pm.details.push(phone.clone());
    let mut new_base = google.clone();
    new_base.details.push(phone.clone());
    let mut pulled = new_base.clone();
    pulled.given_name = "Sam-Sam".into();

    let second = run(&new_base, &pm, &["+447700900105"], &pulled, &stored);
    // Neither side of the frozen field moves, and it is not in what PM changed.
    assert!(second.apply.is_empty(), "nothing applied to PM");
    assert!(
        second.send.is_empty(),
        "PM's name is still unsent, and nothing else is pending"
    );
    assert!(!second.changed_fields().iter().any(|f| f == "given_name"));
    // The entry is carried over, only its google value refreshed; PM's value and date untouched.
    assert_eq!(second.conflicts.len(), 1);
    let c = &second.conflicts[0];
    assert_eq!(c.field, "given_name");
    assert!(c.pm == json!("Samuel"));
    assert!(c.google == json!("Sam-Sam"));
    assert_eq!(c.seen_at, NOW);
    // PM's own value is still PM's: the merge never writes it, and the snapshot is Google's.
    assert_eq!(pm.given_name, "Samuel");
    assert_eq!(second.snapshot.given_name, "Sam-Sam");
}

#[test]
fn a_frozen_field_does_not_stop_the_rest_of_the_person_syncing() {
    let base = card(&[]);
    let (mut pm, mut google) = (base.clone(), base.clone());
    pm.given_name = "Samuel".into();
    google.given_name = "Sammy".into();
    let stored = run(&base, &pm, &[], &google, &[]).conflicts;
    // Google now also adds a phone, and PM changes notes.
    google.details.push(d("phone", "", "+44 7700 900106"));
    pm.notes = "Likes tea".into();
    let o = run(&google_without_phone(&google), &pm, &[], &google, &stored);
    assert!(
        matches!(o.apply.as_slice(), [Apply::AddItem { detail, .. }] if detail.kind == "phone")
    );
    assert_eq!(o.changed_fields(), ["notes"]);
    assert_eq!(o.conflicts.len(), 1);
}

fn google_without_phone(c: &Card) -> Card {
    let mut c = c.clone();
    c.details.retain(|d| d.kind != "phone");
    c
}

#[test]
fn an_open_item_conflict_holds_pms_row_and_follows_googles_replacement() {
    let base = card(&[d("email", "work", "a@example.com")]);
    let pm = card(&[d("email", "work", "b@example.com")]);
    let google = card(&[d("email", "work", "c@example.com")]);
    let stored = run(&base, &pm, &["a@example.com"], &google, &[]).conflicts;

    // A pull later, with the snapshot advanced to Google's copy and Google unchanged: still held.
    let o = run(&google, &pm, &["a@example.com"], &google, &stored);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert_eq!(o.conflicts.len(), 1);
    assert_eq!(o.conflicts[0].google["value"], "c@example.com");
    assert_eq!(o.frozen_rows, [0]);

    // Google then edits c@ to d@: the entry follows it, PM's row is still held.
    let newer = card(&[d("email", "work", "d@example.com")]);
    let o = run(&google, &pm, &["a@example.com"], &newer, &stored);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert_eq!(o.conflicts.len(), 1);
    assert_eq!(o.conflicts[0].google["value"], "d@example.com");
    assert_eq!(o.conflicts[0].pm["value"], "b@example.com");

    // Google removing it leaves the question open, with nothing on Google's side.
    let o = run(&google, &pm, &["a@example.com"], &card(&[]), &stored);
    assert!(o.apply.is_empty() && o.send.is_empty());
    assert!(o.conflicts[0].google.is_null());
}

#[test]
fn an_open_attribute_conflict_freezes_only_that_attribute() {
    let base = card(&[job("Example Works", "Engineer", "Tools")]);
    let pm = card(&[job("Example Works", "Lead", "Tools")]);
    let google = card(&[job("Example Works", "Manager", "Tools")]);
    let stored = run(&base, &pm, &[], &google, &[]).conflicts;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].attribute.as_deref(), Some("parts.title"));

    // Google changes the title again and PM adds a department: the title stays frozen, the
    // department goes out, the entry's google value follows.
    let google2 = card(&[job("Example Works", "Director", "Tools")]);
    let pm2 = card(&[job("Example Works", "Lead", "Platform")]);
    let o = run(&google, &pm2, &[], &google2, &stored);
    assert!(o.apply.is_empty());
    assert!(
        matches!(o.send.as_slice(), [Send::EditItem { attrs, .. }] if attrs.as_slice() == ["parts.department"])
    );
    assert_eq!(o.conflicts.len(), 1);
    assert!(o.conflicts[0].google == json!("Director") && o.conflicts[0].pm == json!("Lead"));
}

#[test]
fn a_conflict_round_trips_through_its_stored_json() {
    let c = Conflict {
        field: "email".into(),
        item_key: Some("a@example.com".into()),
        attribute: Some("label".into()),
        pm: json!("home"),
        google: json!("other"),
        seen_at: NOW.into(),
    };
    let text = serde_json::to_string(&vec![c.clone()]).unwrap();
    let back: Vec<Conflict> = serde_json::from_str(&text).unwrap();
    assert!(back == vec![c]);
    // The optional keys are left out of a field's conflict.
    let single = Conflict {
        field: "notes".into(),
        item_key: None,
        attribute: None,
        pm: json!("a"),
        google: json!("b"),
        seen_at: NOW.into(),
    };
    let v = serde_json::to_value(&single).unwrap();
    assert!(v.get("item_key").is_none() && v.get("attribute").is_none());
    assert_eq!(
        Single::from_word("birthday").map(Single::word),
        Some("birthday")
    );
    assert!(Single::from_word("nope").is_none());
}

// --- the edges ---------------------------------------------------------------------------------------------------

#[test]
fn no_base_treats_both_sides_as_having_added_everything() {
    let mut pm = card(&[d("email", "", "a@example.com")]);
    pm.notes = "Mine".into();
    let google = card(&[
        d("email", "", "a@example.com"),
        d("phone", "", "+44 7700 900107"),
    ]);
    let none = Side {
        card: &pm,
        starred: false,
        src_keys: &[],
    };
    let g = Side {
        card: &google,
        starred: false,
        src_keys: &[],
    };
    let o = merge(None, &none, &g, &[], NOW);
    assert!(o.conflicts.is_empty());
    // The email both have agrees; the phone comes in; PM's notes go out.
    assert!(
        matches!(o.apply.as_slice(), [Apply::AddItem { detail, .. }] if detail.kind == "phone")
    );
    assert_eq!(o.changed_fields(), ["notes"]);
}

#[test]
fn an_unrelated_new_google_item_is_not_swallowed_by_an_edited_vs_removed_conflict() {
    // PM edited a to b; Google removed a and, separately, added a home address of its own.
    let base = card(&[d("email", "work", "a@example.com")]);
    let pm = card(&[d("email", "work", "b@example.com")]);
    let google = card(&[d("email", "home", "x@example.com")]);
    let o = run(&base, &pm, &["a@example.com"], &google, &[]);
    assert_eq!(o.conflicts.len(), 1);
    assert!(
        o.conflicts[0].google.is_null(),
        "x is not a plausible replacement for a: it is a different kind of address"
    );
    assert_eq!(o.conflicts[0].pm["value"], "b@example.com");
    // x reaches PM as a plain addition, and is never offered for deletion.
    assert!(
        matches!(o.apply.as_slice(), [Apply::AddItem { detail, .. }] if detail.value == "x@example.com")
    );
    assert!(o.send.is_empty());
    assert_eq!(o.frozen_rows, [0]);

    // Once PM has x and the conflict is answered "Keep PM's" (the entry gone, PM's row on b),
    // the next merge does not send x back as a removal: PM has it.
    let pm_after = card(&[
        d("email", "work", "b@example.com"),
        d("email", "home", "x@example.com"),
    ]);
    let o = run(&google, &pm_after, &["", ""], &google, &[]);
    assert!(
        !o.send.iter().any(
            |s| matches!(s, Send::RemoveItem { google_key, .. } if google_key == "x@example.com")
        ),
        "x must stay on Google"
    );

    // Two PM edits lost and one new Google item: nothing says which it replaces, so none is offered.
    let base = card(&[
        d("email", "work", "a@example.com"),
        d("email", "work", "c@example.com"),
    ]);
    let pm = card(&[
        d("email", "work", "b@example.com"),
        d("email", "work", "d@example.com"),
    ]);
    let google = card(&[d("email", "work", "z@example.com")]);
    let o = run(
        &base,
        &pm,
        &["a@example.com", "c@example.com"],
        &google,
        &[],
    );
    assert_eq!(o.conflicts.len(), 2);
    assert!(o.conflicts.iter().all(|c| c.google.is_null()));

    // PM edited a; Google removed a and edited c (which PM left alone) into z. z may be c's edit,
    // so it is not offered as a's replacement: a's conflict stays "PM's value vs removed", and z
    // arrives as an addition (c goes with Google's change).
    let base = card(&[
        d("email", "work", "a@example.com"),
        d("email", "work", "c@example.com"),
    ]);
    let pm = card(&[
        d("email", "work", "b@example.com"),
        d("email", "work", "c@example.com"),
    ]);
    let google = card(&[d("email", "work", "z@example.com")]);
    let o = run(
        &base,
        &pm,
        &["a@example.com", "c@example.com"],
        &google,
        &[],
    );
    assert_eq!(o.conflicts.len(), 1);
    assert!(o.conflicts[0].google.is_null());
    assert_eq!(o.conflicts[0].pm["value"], "b@example.com");
    assert!(o
        .apply
        .iter()
        .any(|a| matches!(a, Apply::AddItem { detail, .. } if detail.value == "z@example.com")));
}

#[test]
fn the_same_item_written_two_ways_is_not_a_conflict_without_a_base() {
    let pm = card(&[d("email", "", "Sam@Example.com")]);
    let google = card(&[d("email", "", "sam@example.com")]);
    let pm_side = Side {
        card: &pm,
        starred: false,
        src_keys: &[],
    };
    let g_side = Side {
        card: &google,
        starred: false,
        src_keys: &[],
    };
    let o = merge(None, &pm_side, &g_side, &[], NOW);
    assert!(
        o.conflicts.is_empty(),
        "the key says they are the same address"
    );
    assert!(o.apply.is_empty() && o.send.is_empty());
}

#[test]
fn a_pending_edit_keeps_its_src_key_so_it_stays_an_edit_in_place() {
    let base = card(&[d("email", "work", "a@example.com")]);
    let pm = card(&[d("email", "work", "b@example.com")]);
    let o = run(&base, &pm, &["a@example.com"], &base, &[]);
    assert!(matches!(o.send.as_slice(), [Send::EditItem { .. }]));
    assert!(o.frozen_rows.is_empty());
    assert_eq!(
        o.keep_src_rows(),
        [0],
        "an unsent edit is not reset to its new value"
    );
    // A row that agrees with Google is free to be reset.
    let o = run(&base, &base, &["a@example.com"], &base, &[]);
    assert!(o.keep_src_rows().is_empty());
}

#[test]
fn a_school_compares_its_field_of_study_and_not_a_title() {
    let school = |field: &str| CardDetail {
        parts: Some(json!({ "field": field })),
        ..d("school", "", "Example College")
    };
    let base = card(&[school("Maths")]);
    let o = run(&base, &card(&[school("Physics")]), &[], &base, &[]);
    assert!(o.conflicts.is_empty() && o.apply.is_empty());
    assert!(
        matches!(o.send.as_slice(), [Send::EditItem { attrs, .. }] if attrs.as_slice() == ["parts.field"])
    );
    // Google changes it, PM doesn't: PM's `field` is what is updated.
    let o = run(&base, &base, &[], &card(&[school("Chemistry")]), &[]);
    match o.apply.as_slice() {
        [Apply::EditItem { to, .. }] => {
            assert_eq!(to.parts.as_ref().unwrap()["field"], "Chemistry")
        }
        _ => panic!("one edit to apply"),
    }
}

#[test]
fn the_new_snapshot_is_googles_copy_whatever_happened() {
    let base = card(&[d("email", "", "a@example.com")]);
    let mut pm = base.clone();
    pm.notes = "Mine".into();
    let google = card(&[
        d("email", "", "a@example.com"),
        d("phone", "", "+44 7700 900108"),
    ]);
    let o = run(&base, &pm, &[], &google, &[]);
    assert!(o.snapshot == google);
}
