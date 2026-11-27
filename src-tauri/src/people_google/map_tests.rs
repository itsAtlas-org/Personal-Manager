// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The mapping table of notes 05 section O, both ways, one test per row. Every contact here is
//! invented (example.com addresses, 555-01xx numbers, made-up names).

use super::*;
use serde_json::json;

fn of<'a>(card: &'a Card, kind: &str) -> Vec<&'a CardDetail> {
    card.details.iter().filter(|d| d.kind == kind).collect()
}

fn read(person: &Value) -> Mapped {
    read_person(person).expect("a valid contact")
}

fn card_of(person: &Value) -> Card {
    read(person).card.expect("a contact with a name")
}

fn detail(kind: &str, label: &str, value: &str) -> CardDetail {
    CardDetail {
        kind: kind.into(),
        label: label.into(),
        value: value.into(),
        ..Default::default()
    }
}

/// A contact with something in every field PM maps, and Google-only attributes besides.
fn full() -> Value {
    json!({
        "resourceName": "people/c1234567890",
        "etag": "%EgUBAj0DNy4=",
        "metadata": {
            "sources": [
                { "type": "CONTACT", "id": "1234567890", "etag": "etag-contact-1" },
                { "type": "PROFILE", "id": "999", "etag": "etag-profile" }
            ],
            "previousResourceNames": ["people/c1112223334", "not-a-contact"]
        },
        "names": [
            { "givenName": "Sam", "middleName": "Quinn", "familyName": "Example",
              "honorificPrefix": "Dr.", "honorificSuffix": "Jr.", "phoneticGivenName": "Sahm",
              "displayName": "Dr. Sam Quinn Example Jr." },
            { "givenName": "Second" }
        ],
        "nicknames": [
            { "value": "Sammy", "type": "DEFAULT" },
            { "value": "Sam Old", "type": "MAIDEN_NAME" },
            { "value": "Sx", "type": "SHORT_NAME" }
        ],
        "birthdays": [{ "date": { "year": 1990, "month": 4, "day": 15 }, "text": "April 15" }],
        "events": [
            { "date": { "year": 2015, "month": 6, "day": 1 }, "type": "anniversary", "formattedType": "Jubilee" },
            { "date": { "month": 12, "day": 24 }, "type": "Name day" }
        ],
        "emailAddresses": [
            { "value": "sam@example.com", "type": "work", "formattedType": "Work", "displayName": "Sam at work",
              "metadata": { "primary": true } },
            { "value": "sam.home@example.com", "type": "home" }
        ],
        "phoneNumbers": [
            { "value": "+44 7700 900101", "type": "mobile", "canonicalForm": "+447700900101" },
            { "value": "020 7946 0102", "type": "Studio line" }
        ],
        "addresses": [{
            "streetAddress": "1 Example Road", "extendedAddress": "Flat 2", "poBox": "PO 7",
            "city": "Sampleton", "region": "Exshire", "postalCode": "EX1 2AB", "country": "Exland",
            "countryCode": "XX", "type": "home", "formattedValue": "stale text"
        }],
        "organizations": [
            { "name": "Example Works", "title": "Engineer", "department": "Tools", "type": "work",
              "startDate": { "year": 2019, "month": 3 }, "endDate": { "year": 2022 }, "current": false,
              "costCenter": "CC-9", "phoneticName": "Egg-zample" },
            { "name": "Example College", "title": "Maths", "type": "school",
              "startDate": { "year": 2008 }, "endDate": { "year": 2011 } },
            { "name": "Sample Club", "type": "Volunteer", "current": true }
        ],
        "urls": [
            { "value": "https://example.com/sam", "type": "homePage", "formattedType": "Home page" },
            { "value": "https://example.com/sam-blog", "type": "Instagram" }
        ],
        "imClients": [
            { "username": "sam#0001", "protocol": "Discord", "formattedProtocol": "Disc" }
        ],
        "relations": [{ "person": "Pat Example", "type": "spouse", "formattedType": "Partner" }],
        "biographies": [{ "value": "Met at the sample fair.", "contentType": "TEXT_PLAIN" }],
        "userDefined": [{ "key": "Allergy", "value": "Peanuts" }],
        "memberships": [
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/myContacts" } },
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/starred" } },
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/friends" } },
            { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/6a2b3c4d5e" } },
            { "domainMembership": { "inViewerDomain": true } }
        ],
        "clientData": [
            { "key": "other-app", "value": "x" },
            { "key": "pm.uid", "value": "0123456789abcdef0123456789abcdef" }
        ],
        "genders": [{ "value": "unspecified" }],
        "photos": [{ "url": "https://example.com/p.jpg" }],
        "locations": [{ "value": "Desk 4", "type": "desk" }]
    })
}

// --- the consts ---------------------------------------------------------------------------------

#[test]
fn the_one_read_mask_and_sources_pair() {
    assert_eq!(
        READ_MASK,
        "names,nicknames,birthdays,events,emailAddresses,phoneNumbers,addresses,organizations,urls,imClients,relations,biographies,userDefined,memberships,clientData,metadata"
    );
    assert_eq!(SOURCES, "READ_SOURCE_TYPE_CONTACT");
    // Nothing PM never reads is asked for: photos, genders, locations and Google's profile data.
    for never in [
        "photos",
        "genders",
        "locations",
        "PROFILE",
        "externalIds",
        "sipAddresses",
    ] {
        assert!(
            !READ_MASK.contains(never) && !SOURCES.contains(never),
            "{never}"
        );
    }
    // Every Person array the mapping reads is in the mask.
    for field in MAPPED_KINDS.iter().flat_map(|k| fields_of(k)) {
        assert!(READ_MASK.split(',').any(|m| m == *field), "{field}");
    }
}

// --- one test per row of section O, Google to PM -----------------------------------------------------

#[test]
fn the_name_is_names_zero_only_and_keeps_google_only_parts_out() {
    let card = card_of(&full());
    assert_eq!(card.given_name, "Sam");
    assert_eq!(card.middle_name, "Quinn");
    assert_eq!(card.family_name, "Example");
    assert_eq!(card.name_prefix, "Dr.");
    assert_eq!(card.name_suffix, "Jr.");
    // Composed by PM's rule; Google's displayName is output only and not read.
    assert_eq!(card.display_name, "Dr. Sam Quinn Example Jr.");

    // A name Google holds as one free string is the display name.
    let one = json!({ "resourceName": "people/c1", "names": [{ "unstructuredName": "Example Works Ltd" }] });
    let card = card_of(&one);
    assert_eq!(card.display_name, "Example Works Ltd");
    assert!(card.given_name.is_empty() && card.family_name.is_empty());
}

#[test]
fn a_nickname_is_a_nickname_and_a_maiden_name_is_a_former_name() {
    let card = card_of(&full());
    let nicks: Vec<&str> = of(&card, "nickname")
        .iter()
        .map(|d| d.value.as_str())
        .collect();
    assert_eq!(nicks, ["Sammy", "Sx"]);
    let former: Vec<&str> = of(&card, "former_name")
        .iter()
        .map(|d| d.value.as_str())
        .collect();
    assert_eq!(former, ["Sam Old"]);
    // A nickname has no label: the type is Google's closed enum.
    assert!(of(&card, "nickname").iter().all(|d| d.label.is_empty()));
}

#[test]
fn emails_and_phones_keep_their_type_as_the_label_and_never_the_formatted_one() {
    let card = card_of(&full());
    let emails = of(&card, "email");
    assert_eq!(emails.len(), 2);
    assert_eq!(
        (emails[0].value.as_str(), emails[0].label.as_str()),
        ("sam@example.com", "work")
    );
    assert_eq!(emails[1].label, "home");
    let phones = of(&card, "phone");
    assert_eq!(
        (phones[0].value.as_str(), phones[0].label.as_str()),
        ("+44 7700 900101", "mobile")
    );
    // A custom type is the label as typed.
    assert_eq!(phones[1].label, "Studio line");
}

#[test]
fn the_primary_item_of_an_array_comes_first() {
    let p = json!({ "resourceName": "people/c1", "names": [{ "givenName": "Sam" }],
        "emailAddresses": [
            { "value": "a@example.com" },
            { "value": "b@example.com", "metadata": { "primary": true } },
            { "value": "c@example.com" }
        ] });
    let card = card_of(&p);
    let order: Vec<&str> = of(&card, "email")
        .iter()
        .map(|d| d.value.as_str())
        .collect();
    assert_eq!(order, ["b@example.com", "a@example.com", "c@example.com"]);
}

#[test]
fn an_address_is_its_parts_and_its_text() {
    let card = card_of(&full());
    let a = of(&card, "address");
    assert_eq!(a.len(), 1);
    let parts = a[0].parts.as_ref().unwrap();
    assert_eq!(parts["street"], "1 Example Road");
    assert_eq!(parts["extended"], "Flat 2");
    assert_eq!(parts["po_box"], "PO 7");
    assert_eq!(parts["city"], "Sampleton");
    assert_eq!(parts["region"], "Exshire");
    assert_eq!(parts["postal_code"], "EX1 2AB");
    assert_eq!(parts["country"], "Exland");
    // The text is composed from the parts, not Google's (possibly stale) formattedValue.
    assert!(a[0]
        .value
        .starts_with("1 Example Road\nFlat 2\nPO 7\nSampleton Exshire EX1 2AB"));
    assert!(!a[0].value.contains("stale"));
    assert_eq!(a[0].label, "home");

    // An address that is only a street is flat, as a file reads one; one with only a one-line
    // value is that text.
    let flat = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }],
        "addresses": [{ "streetAddress": "9 Short Lane" }, { "formattedValue": "Somewhere\nElse" }] });
    let card = card_of(&flat);
    let a = of(&card, "address");
    assert!(a[0].parts.is_none() && a[0].value == "9 Short Lane");
    assert!(a[1].parts.is_none() && a[1].value == "Somewhere\nElse");
}

#[test]
fn urls_and_im_clients_are_web_labelled_by_type_or_protocol() {
    let card = card_of(&full());
    let web = of(&card, "web");
    assert_eq!(web.len(), 3);
    assert_eq!(
        (web[0].value.as_str(), web[0].label.as_str()),
        ("https://example.com/sam", "homePage")
    );
    assert_eq!(web[1].label, "Instagram");
    assert_eq!(
        (web[2].value.as_str(), web[2].label.as_str()),
        ("sam#0001", "Discord")
    );
    // Never the localised text.
    assert!(web
        .iter()
        .all(|d| d.label != "Home page" && d.label != "Disc"));
}

#[test]
fn a_job_a_school_and_a_custom_type_are_told_apart() {
    let card = card_of(&full());
    let work = of(&card, "work");
    assert_eq!(work.len(), 2);
    assert_eq!(work[0].value, "Example Works");
    let parts = work[0].parts.as_ref().unwrap();
    assert_eq!(
        (parts["title"].as_str(), parts["department"].as_str()),
        (Some("Engineer"), Some("Tools"))
    );
    assert_eq!(work[0].from_date.as_deref(), Some("2019-03"));
    assert_eq!(work[0].to_date.as_deref(), Some("2022"));
    assert!(!work[0].is_current);
    assert!(work[0].label.is_empty());
    // Any type that isn't school is a job, and a custom one is its label.
    assert_eq!(work[1].value, "Sample Club");
    assert_eq!(work[1].label, "Volunteer");
    assert!(work[1].is_current);

    let school = of(&card, "school");
    assert_eq!(school.len(), 1);
    assert_eq!(school[0].value, "Example College");
    // A school's field of study is PM's `parts.field`, never `title` (editor, vCard, dedupe).
    let parts = school[0].parts.as_ref().unwrap();
    assert_eq!(parts["field"], "Maths");
    assert!(parts.get("title").is_none());
    assert_eq!(school[0].from_date.as_deref(), Some("2008"));
}

#[test]
fn custom_fields_events_and_relations() {
    let card = card_of(&full());
    let custom = of(&card, "custom");
    assert_eq!(
        (custom[0].label.as_str(), custom[0].value.as_str()),
        ("Allergy", "Peanuts")
    );
    let dates = of(&card, "date");
    assert_eq!(
        (dates[0].value.as_str(), dates[0].label.as_str()),
        ("2015-06-01", "anniversary")
    );
    // Year 0 or absent is a year-less date.
    assert_eq!(
        (dates[1].value.as_str(), dates[1].label.as_str()),
        ("--12-24", "Name day")
    );
    let rel = of(&card, "relation");
    assert_eq!(
        (rel[0].value.as_str(), rel[0].label.as_str()),
        ("Pat Example", "spouse")
    );
}

#[test]
fn a_birthday_with_no_year_or_a_zero_year_has_no_year() {
    for date in [
        json!({ "month": 4, "day": 15 }),
        json!({ "year": 0, "month": 4, "day": 15 }),
    ] {
        let p = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }], "birthdays": [{ "date": date }] });
        assert_eq!(card_of(&p).born_on.as_deref(), Some("--04-15"));
    }
    assert_eq!(card_of(&full()).born_on.as_deref(), Some("1990-04-15"));
    // 29 February is valid with or without a year.
    let leap = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }],
        "birthdays": [{ "date": { "month": 2, "day": 29 } }] });
    assert_eq!(card_of(&leap).born_on.as_deref(), Some("--02-29"));
}

#[test]
fn a_birthday_held_only_as_text_becomes_a_custom_detail() {
    let p = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }],
        "birthdays": [{ "text": "the first of May" }] });
    let card = card_of(&p);
    assert!(card.born_on.is_none());
    let custom = of(&card, "custom");
    assert_eq!(custom.len(), 1);
    assert_eq!(custom[0].label, BIRTHDAY_AS_WRITTEN);
    assert_eq!(custom[0].value, "the first of May");
    // A year-only date can't be a PM birthday and has no text: nothing is invented.
    let year_only = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }],
        "birthdays": [{ "date": { "year": 1990 } }] });
    let card2 = card_of(&year_only);
    assert!(card2.born_on.is_none() && of(&card2, "custom").is_empty());
    // That detail can't be written back: Google's text field is deprecated.
    assert!(new_item(custom[0]).is_none());
}

#[test]
fn notes_are_the_first_biography_with_html_made_plain() {
    assert_eq!(card_of(&full()).notes, "Met at the sample fair.");
    let html = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }],
        "biographies": [{ "contentType": "TEXT_HTML",
            "value": "<p>Hello&nbsp;<b>Sam</b> &amp; Pat</p><p>Second<br>line &lt;3 &#33;</p><script>alert(1)</script><style>p{}</style>" }] });
    assert_eq!(card_of(&html).notes, "Hello Sam & Pat\nSecond\nline <3 !");
    // Plain text is kept as it is, entities and all.
    let plain = json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }],
        "biographies": [{ "contentType": "TEXT_PLAIN", "value": "1 < 2 &amp; 3" }] });
    assert_eq!(card_of(&plain).notes, "1 < 2 &amp; 3");
}

#[test]
fn html_stripping_handles_the_odd_cases() {
    assert_eq!(strip_html("a < b"), "a < b");
    assert_eq!(strip_html("<div>one</div><div>two</div>"), "one\ntwo");
    assert_eq!(strip_html("x&bogus;y &#x41;&#66;"), "x&bogus;y AB");
    assert_eq!(strip_html("<script>never"), "");
    assert_eq!(strip_html("plain"), "plain");
}

#[test]
fn memberships_are_the_star_and_the_user_groups_only() {
    let m = read(&full());
    assert!(m.starred);
    // myContacts is implicit, friends is a system group, the domain membership isn't a group.
    assert_eq!(m.groups, ["contactGroups/6a2b3c4d5e"]);
    assert!(is_system_group("contactGroups/friends"));
    assert!(is_system_group("contactGroups/myContacts"));
    assert!(!is_system_group("contactGroups/6a2b3c4d5e"));
    // A malformed group name is no group.
    let odd = json!({ "resourceName": "people/c1", "memberships": [
        { "contactGroupMembership": { "contactGroupResourceName": "contactGroups/a/b" } }] });
    assert!(read(&odd).groups.is_empty());
}

#[test]
fn what_pm_does_not_model_is_never_read() {
    let card = card_of(&full());
    // Genders, photos, locations, phonetic names, cost centres, country codes: nowhere in the card.
    assert!(of(&card, "location").is_empty());
    let all: String = serde_json::to_string(&card).unwrap();
    for never in [
        "unspecified",
        "p.jpg",
        "Desk 4",
        "Sahm",
        "CC-9",
        "Egg-zample",
        "\"XX\"",
        "Sam at work",
    ] {
        assert!(!all.contains(never), "{never}");
    }
    // The only id on the card is the one PM put on the contact.
    assert_eq!(
        card.ident.as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
}

#[test]
fn the_ids_and_the_etag_come_from_the_contact_source() {
    let m = read(&full());
    assert_eq!(m.resource_name, "people/c1234567890");
    assert_eq!(m.contact_id.as_deref(), Some("1234567890"));
    assert_eq!(m.etag.as_deref(), Some("etag-contact-1"));
    // A previous resource name that isn't a contact's is dropped.
    assert_eq!(m.previous_names, ["people/c1112223334"]);
    assert_eq!(m.ident.as_deref(), Some("0123456789abcdef0123456789abcdef"));
    assert!(!m.deleted && !m.may_be_cut);

    // The profile source's etag is never the one sent.
    let profile_only = json!({ "resourceName": "people/c5",
        "metadata": { "sources": [{ "type": "PROFILE", "id": "1", "etag": "p" }] } });
    assert!(read(&profile_only).etag.is_none());
}

#[test]
fn a_deleted_entry_has_ids_and_no_card() {
    let gone = json!({ "resourceName": "people/c77", "metadata": { "deleted": true, "objectType": "PERSON" } });
    let m = read(&gone);
    assert!(m.deleted && m.card.is_none());
    assert_eq!(m.resource_name, "people/c77");
    // A resource name that isn't a contact's is no contact.
    for bad in [
        "people/me",
        "people/c",
        "people/c12x",
        "contactGroups/1",
        "people/c1/x",
    ] {
        assert!(
            read_person(&json!({ "resourceName": bad })).is_none(),
            "{bad}"
        );
    }
    assert!(read_person(&json!({})).is_none());
}

#[test]
fn a_contact_with_nothing_to_call_it_by_has_no_card() {
    let empty = json!({ "resourceName": "people/c9", "biographies": [{ "value": "just a note" }] });
    assert!(read(&empty).card.is_none());
    // An email alone is enough to call someone by.
    let email =
        json!({ "resourceName": "people/c9", "emailAddresses": [{ "value": "x@example.com" }] });
    assert_eq!(card_of(&email).display_name, "x@example.com");
}

#[test]
fn exactly_a_hundred_emails_or_numbers_may_have_been_cut() {
    let many = |field: &str, n: usize| {
        let items: Vec<Value> = (0..n)
            .map(|i| json!({ "value": format!("v{i}@example.com") }))
            .collect();
        json!({ "resourceName": "people/c1", "names": [{ "givenName": "A" }], field: items })
    };
    assert!(read(&many("emailAddresses", 100)).may_be_cut);
    assert!(read(&many("phoneNumbers", 100)).may_be_cut);
    assert!(!read(&many("emailAddresses", 99)).may_be_cut);
}

#[test]
fn values_are_cleaned_and_bounded_like_every_other_card() {
    let long = "x".repeat(2000);
    let p = json!({ "resourceName": "people/c1",
        "names": [{ "givenName": "  Sam\u{202e}  " }],
        "emailAddresses": [{ "value": long }, { "value": "   " }] });
    let card = card_of(&p);
    assert_eq!(card.given_name, "Sam");
    let emails = of(&card, "email");
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].value.chars().count(), 1024);
}

// --- PM to Google, the same rows ------------------------------------------------------------------------

#[test]
fn every_kind_writes_to_its_google_field() {
    let cases = [
        (
            detail("email", "home", "a@example.com"),
            "emailAddresses",
            json!({ "value": "a@example.com", "type": "home" }),
        ),
        (
            detail("phone", "", "+44 7700 900103"),
            "phoneNumbers",
            json!({ "value": "+44 7700 900103" }),
        ),
        (
            detail("web", "Instagram", "https://example.com/i"),
            "urls",
            json!({ "value": "https://example.com/i", "type": "Instagram" }),
        ),
        (
            detail("nickname", "", "Sammy"),
            "nicknames",
            json!({ "value": "Sammy", "type": "DEFAULT" }),
        ),
        (
            detail("former_name", "", "Old Name"),
            "nicknames",
            json!({ "value": "Old Name", "type": "MAIDEN_NAME" }),
        ),
        (
            detail("relation", "friend", "Pat Example"),
            "relations",
            json!({ "person": "Pat Example", "type": "friend" }),
        ),
        (
            detail("custom", "Allergy", "Peanuts"),
            "userDefined",
            json!({ "key": "Allergy", "value": "Peanuts" }),
        ),
        (
            detail("date", "anniversary", "2015-06-01"),
            "events",
            json!({ "date": { "year": 2015, "month": 6, "day": 1 }, "type": "anniversary" }),
        ),
        (
            detail("date", "", "--12-24"),
            "events",
            json!({ "date": { "month": 12, "day": 24 } }),
        ),
    ];
    for (d, field, want) in cases {
        let (got_field, item) = new_item(&d).unwrap();
        assert_eq!(got_field, field, "{}", d.kind);
        assert!(item == want, "{}", d.kind);
    }
    // Where they are now is PM's own: it has no Google field.
    assert!(new_item(&detail("location", "", "Lisbon")).is_none());
}

#[test]
fn a_job_and_a_school_write_with_their_types_dates_and_parts() {
    let job = CardDetail {
        parts: Some(json!({ "title": "Engineer", "department": "Tools" })),
        from_date: Some("2019-03".into()),
        to_date: Some("2022".into()),
        is_current: true,
        ..detail("work", "", "Example Works")
    };
    let (field, item) = new_item(&job).unwrap();
    assert_eq!(field, "organizations");
    assert!(
        item == json!({ "name": "Example Works", "title": "Engineer", "department": "Tools",
        "startDate": { "year": 2019, "month": 3 }, "endDate": { "year": 2022 }, "current": true, "type": "work" })
    );
    // A school is always typed school, or Google (and PM, reading it back) would take it for a job.
    let school = detail("school", "Night class", "Example College");
    assert_eq!(new_item(&school).unwrap().1["type"], "school");
    // Its field of study goes out as Google's `title`, and it has no department.
    let school = CardDetail {
        parts: Some(json!({ "field": "Maths", "department": "ignored" })),
        ..detail("school", "", "Example College")
    };
    let item = new_item(&school).unwrap().1;
    assert_eq!(item["title"], "Maths");
    assert!(item.get("department").is_none());
    // A custom job type is kept.
    assert_eq!(
        new_item(&detail("work", "Volunteer", "Sample Club"))
            .unwrap()
            .1["type"],
        "Volunteer"
    );
}

#[test]
fn an_address_writes_its_parts_or_its_text() {
    let parts = CardDetail {
        parts: Some(
            json!({ "street": "1 Example Road", "city": "Sampleton", "po_box": "PO 7", "postal_code": "EX1 2AB" }),
        ),
        ..detail("address", "home", "1 Example Road\nPO 7\nSampleton EX1 2AB")
    };
    let (_, item) = new_item(&parts).unwrap();
    assert!(
        item == json!({ "streetAddress": "1 Example Road", "city": "Sampleton", "poBox": "PO 7",
        "postalCode": "EX1 2AB", "type": "home" })
    );
    assert_eq!(
        new_item(&detail("address", "", "9 Short Lane")).unwrap().1["streetAddress"],
        "9 Short Lane"
    );
    assert_eq!(
        new_item(&detail("address", "", "Somewhere\nElse"))
            .unwrap()
            .1["formattedValue"],
        "Somewhere\nElse"
    );
}

#[test]
fn a_new_contact_carries_everything_mapped_and_its_pm_id() {
    let card = card_of(&full());
    let body = person_for_create(
        &card,
        true,
        &["contactGroups/6a2b3c4d5e".to_string()],
        "feedfacefeedfacefeedfacefeedface",
    );
    assert!(
        body["clientData"]
            == json!([{ "key": "pm.uid", "value": "feedfacefeedfacefeedfacefeedface" }])
    );
    let groups: Vec<&str> = body["memberships"]
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
        groups,
        [
            "contactGroups/myContacts",
            "contactGroups/starred",
            "contactGroups/6a2b3c4d5e"
        ]
    );
    assert_eq!(body["names"][0]["givenName"], "Sam");
    assert!(body["names"].as_array().unwrap().len() == 1);
    assert_eq!(body["birthdays"][0]["date"]["year"], 1990);
    assert_eq!(body["biographies"][0]["contentType"], "TEXT_PLAIN");
    // Only the writable fields: nothing output-only, nothing PM invented.
    for never in ["metadata", "photos", "locations", "etag", "resourceName"] {
        assert!(body.get(never).is_none(), "{never}");
    }

    // A company-only card is named by its one free string.
    let company = Card {
        display_name: "Example Works Ltd".into(),
        ..Default::default()
    };
    let body = person_for_create(&company, false, &[], "u");
    assert_eq!(body["names"][0]["unstructuredName"], "Example Works Ltd");
    assert!(body.get("birthdays").is_none() && body.get("biographies").is_none());
}

#[test]
fn what_pm_writes_reads_back_as_the_same_card() {
    // PM to Google to PM, for every kind: the table's round trip.
    let card = card_of(&full());
    let mut person = person_for_create(&card, true, &[], "feedfacefeedfacefeedfacefeedface");
    person["resourceName"] = json!("people/c42");
    let back = card_of(&person);
    assert_eq!(back.given_name, card.given_name);
    assert_eq!(back.family_name, card.family_name);
    assert_eq!(back.name_prefix, card.name_prefix);
    assert_eq!(back.born_on, card.born_on);
    assert_eq!(back.notes, card.notes);
    for kind in MAPPED_KINDS {
        let (a, b) = (of(&card, kind), of(&back, kind));
        assert_eq!(a.len(), b.len(), "{kind}");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.value, y.value, "{kind}");
            assert_eq!(x.label, y.label, "{kind}");
            assert!(x.parts == y.parts, "{kind}");
            assert!(
                (&x.from_date, &x.to_date, x.is_current)
                    == (&y.from_date, &y.to_date, y.is_current),
                "{kind}"
            );
        }
    }
    assert!(read(&person).starred);
}

#[test]
fn an_unlabelled_custom_field_reads_back_unlabelled() {
    let (field, item) = new_item(&detail("custom", "", "Peanuts")).unwrap();
    assert_eq!(field, "userDefined");
    assert_eq!(item["key"], "Custom");
    let back = detail_of("userDefined", &item).unwrap();
    assert_eq!((back.label.as_str(), back.value.as_str()), ("", "Peanuts"));
}

#[test]
fn a_name_write_keeps_the_parts_pm_does_not_model() {
    let existing = json!({ "givenName": "Sam", "phoneticGivenName": "Sahm", "unstructuredName": "Sam Example",
        "displayName": "Sam Example", "metadata": { "primary": true } });
    let card = Card {
        given_name: "Samuel".into(),
        family_name: "Example".into(),
        ..Default::default()
    };
    let out = write_name(Some(&existing), &card);
    assert_eq!(out["givenName"], "Samuel");
    assert_eq!(out["phoneticGivenName"], "Sahm");
    for gone in ["displayName", "unstructuredName", "metadata"] {
        assert!(out.get(gone).is_none(), "{gone}");
    }
}

#[test]
fn set_attr_edits_one_attribute_and_leaves_the_rest() {
    let mut item = json!({ "value": "a@example.com", "type": "work", "displayName": "A at work", "formattedType": "Work" });
    let to = detail("email", "home", "b@example.com");
    assert!(set_attr("emailAddresses", &mut item, "value", &to));
    assert!(
        item == json!({ "value": "b@example.com", "type": "work", "displayName": "A at work", "formattedType": "Work" })
    );
    assert!(set_attr("emailAddresses", &mut item, "label", &to));
    assert_eq!(item["type"], "home");
    assert!(item.get("formattedType").is_none());
    assert_eq!(item["displayName"], "A at work");
    // Writing what is already there changes nothing.
    assert!(!set_attr("emailAddresses", &mut item, "value", &to));

    let mut phone = json!({ "value": "1", "canonicalForm": "+1" });
    set_attr(
        "phoneNumbers",
        &mut phone,
        "value",
        &detail("phone", "", "2"),
    );
    assert!(phone == json!({ "value": "2" }));

    let mut addr = json!({ "streetAddress": "1 A", "countryCode": "XX", "poBox": "P", "formattedValue": "old" });
    let to = CardDetail {
        parts: Some(json!({ "street": "2 B", "city": "C" })),
        ..detail("address", "", "2 B\nC")
    };
    set_attr("addresses", &mut addr, "parts.street", &to);
    assert!(addr == json!({ "streetAddress": "2 B", "countryCode": "XX", "poBox": "P" }));

    let mut org = json!({ "name": "A", "title": "T", "costCenter": "CC", "type": "school" });
    let to = CardDetail {
        parts: Some(json!({ "field": "F" })),
        is_current: true,
        ..detail("school", "", "B")
    };
    set_attr("organizations", &mut org, "value", &to);
    // A PM school-field edit lands on Google's `title`, in place.
    assert!(set_attr("organizations", &mut org, "parts.field", &to));
    set_attr("organizations", &mut org, "current", &to);
    set_attr("organizations", &mut org, "label", &to);
    assert!(
        org == json!({ "name": "B", "title": "F", "costCenter": "CC", "type": "school", "current": true })
    );
    // Clearing the field drops Google's title.
    let to = detail("school", "", "B");
    assert!(set_attr("organizations", &mut org, "parts.field", &to));
    assert!(org.get("title").is_none());

    // An empty custom label is written as the same word a create uses, never an empty key.
    let mut custom = json!({ "key": "Allergy", "value": "Peanuts" });
    assert!(set_attr(
        "userDefined",
        &mut custom,
        "label",
        &detail("custom", "", "Peanuts")
    ));
    assert_eq!(custom["key"], "Custom");
}

// --- the snapshot ---------------------------------------------------------------------------------------

#[test]
fn a_snapshot_round_trips_and_keeps_the_contact_source_id() {
    let mapped = read(&full());
    let snap = Snap::of(&mapped, vec!["Friends".to_string()]).unwrap();
    assert!(snap.starred);
    let text = snap.to_json();
    let back = Snap::parse(&text).unwrap();
    assert!(back.card == snap.card);
    assert_eq!(back.card.labels, ["Friends"]);
    // The id `people::sources` keeps in a tombstone is read from the same snapshot.
    let tomb: Value = serde_json::from_str(&crate::people::sources::tombstone_snapshot(
        Some(&text),
        &["ident-1".to_string()],
    ))
    .unwrap();
    assert_eq!(tomb["source_id"], "1234567890");
    assert!(Snap::parse("{not json").is_none());
    // A deleted entry has no snapshot.
    let gone = json!({ "resourceName": "people/c77", "metadata": { "deleted": true } });
    assert!(Snap::of(&read(&gone), vec![]).is_none());
}

#[test]
fn the_one_way_door_strings_are_pinned() {
    // Strings other code and stored rows depend on: changing one is a migration, not an edit.
    assert_eq!(BIRTHDAY_AS_WRITTEN, "Birthday (as written)");
    assert_eq!(STARRED_GROUP, "contactGroups/starred");
    assert_eq!(MY_CONTACTS_GROUP, "contactGroups/myContacts");
    assert_eq!(PM_UID_CLIENT_DATA_KEY, "pm.uid");
    assert_eq!(MAPPED_KINDS.len(), 11);
    for kind in MAPPED_KINDS {
        assert!(crate::people::DETAIL_KINDS.contains(&kind), "{kind}");
        assert!(!fields_of(kind).is_empty(), "{kind}");
    }
    assert!(!MAPPED_KINDS.contains(&"location"));
}
