// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where PM thinks this computer is, for one rule: a free Google Gemini key is not used in the United
//! Kingdom, the European Economic Area or Switzerland.
//!
//! Google's terms allow only paid Gemini services for apps used there, and Google's servers don't
//! enforce it (a free key in London gets a normal reply), so PM does. Bobby, 2026-11-26: "pm can
//! refuse since warning is useless if it cant do anything right".
//!
//! THREAT MODEL. This is a guard against honest mistakes (a free key pasted in London by someone who
//! didn't know the rule), NOT a geofence. A person who sets their computer's time zone to Tokyo is
//! outside it, and nothing here tries to stop them. What it must do is fail CLOSED on anything it can't
//! place: an unreadable zone, UTC, a zone id newer than PM's lists. Refusing a free key wrongly costs
//! one paid-key paste or one time-zone change; accepting wrongly breaks Google's terms by PM's own
//! rule.
//!
//! The only source is the operating system's time zone id, read by [`assumed`] (the one impure line).
//! No IP address is looked up and nothing leaves the machine. On Linux that is the `/etc/localtime` link, so the `TZ`
//! variable does not change it. The webview's own zone (a Settings value the person can override) is
//! deliberately not consulted: it says nothing about the computer and a stale one would refuse a key
//! for no reason. The rule is read when a key is saved, when state is built and at every send, and is
//! never cached, so a person who travels is placed wherever the OS follows them.

use serde::Serialize;

use crate::llm_gateway::CloudProvider;

/// Which restricted area a zone is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegionKind {
    Uk,
    Eea,
    Switzerland,
}

impl RegionKind {
    fn token(self) -> &'static str {
        match self {
            RegionKind::Uk => "uk",
            RegionKind::Eea => "eea",
            RegionKind::Switzerland => "switzerland",
        }
    }

    fn label(self) -> &'static str {
        match self {
            RegionKind::Uk => "the United Kingdom",
            RegionKind::Eea => "the European Economic Area",
            RegionKind::Switzerland => "Switzerland",
        }
    }
}

/// What PM concluded about where this computer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Region {
    /// In an area where only paid Google keys may be used. `zone` is the id PM read.
    Restricted { kind: RegionKind, zone: String },
    /// Positively somewhere else.
    Outside,
    /// PM can't tell (UTC, an `Etc/` offset, an id it doesn't know, nothing readable). Treated like
    /// Restricted for a free key, with the zone named so the person can fix it.
    Unplaceable { zone: String },
}

/// Why the verdict that was refused reads as free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Basis {
    /// Google said so (a free-tier answer or a free-tier 429).
    Free,
    /// Nothing was asked, or the check couldn't decide: unknown reads as free.
    Unknown,
}

/// What the webview and the send seam are told about a refusal. Strings, so the TS union is open.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct RegionRefusal {
    /// `uk`, `eea`, `switzerland` or `unplaceable`.
    pub kind: String,
    /// "the United Kingdom", "the European Economic Area", "Switzerland", "somewhere PM can't place".
    pub label: String,
    /// The time zone id PM read; empty when it couldn't read one.
    pub zone: String,
    /// `free` or `unknown`.
    pub basis: String,
}

impl RegionRefusal {
    /// The sentence a refused call and a paused role say. Starts "Gemini is paused:" so a chat reply or
    /// a background log names what happened.
    pub(crate) fn sentence(&self, p: CloudProvider) -> String {
        let who = match p {
            CloudProvider::Gemini => "Gemini",
            other => other.display_name(),
        };
        let why = if self.basis == "free" {
            "Google says this key is on the free tier"
        } else {
            "PM can't confirm this key is paid, so it treats it as free"
        };
        let fix =
            "Make the project paid in AI Studio, then press Check again in Settings > AI & Models.";
        if self.kind == "unplaceable" {
            let zone = if self.zone.is_empty() {
                "unknown"
            } else {
                &self.zone
            };
            format!(
                "{who} is paused: {why}, and PM can't tell where this computer is (its time zone reads as {zone}), so it only uses paid {who} keys. Set your time zone, or make the project paid, then press Check again in Settings > AI & Models."
            )
        } else {
            format!(
                "{who} is paused: {why}, and PM only uses paid {who} keys in {} (it read your time zone as {}). {fix}",
                self.label, self.zone
            )
        }
    }
}

impl Region {
    /// The refusal for this place, `None` outside: a free or unknown key is refused inside an area and
    /// wherever PM can't place the computer.
    pub(crate) fn refusal(&self, basis: Basis) -> Option<RegionRefusal> {
        let basis = match basis {
            Basis::Free => "free",
            Basis::Unknown => "unknown",
        }
        .to_string();
        match self {
            Region::Restricted { kind, zone } => Some(RegionRefusal {
                kind: kind.token().into(),
                label: kind.label().into(),
                zone: zone.clone(),
                basis,
            }),
            Region::Unplaceable { zone } => Some(RegionRefusal {
                kind: "unplaceable".into(),
                label: "somewhere PM can't place".into(),
                zone: zone.clone(),
                basis,
            }),
            Region::Outside => None,
        }
    }
}

/// Where PM assumes this computer is, from the OS time zone. The ONLY impure line here.
pub(crate) fn assumed() -> Region {
    region_of(iana_time_zone::get_timezone().ok().as_deref())
}

/// [`region_of_on`] for the platform PM is running on.
pub(crate) fn region_of(zone: Option<&str>) -> Region {
    region_of_on(zone, cfg!(windows))
}

/// The whole rule, pure. Anything PM can't place is `Unplaceable`: [`classify`] answers `None` for an
/// id that fell through every list, so a zone a newer system produces is never read as "outside".
pub(crate) fn region_of_on(zone: Option<&str>, windows: bool) -> Region {
    let zone = zone.map(normalise).unwrap_or_default();
    classify(&zone, windows).unwrap_or(Region::Unplaceable { zone })
}

/// Trim, and strip one leading `posix/` or `right/` (Linux hands back the link target as it is, so
/// `posix/Europe/London` is a possible id). The id comes off the OS (a link target), so it is cut with
/// `get`: a multi-byte character straddling the prefix length must read as "no prefix" and fall
/// through to Unplaceable, never panic inside a settings read that every user makes.
fn normalise(zone: &str) -> String {
    let z = zone.trim();
    for prefix in ["posix/", "right/"] {
        let has = z
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix));
        if has && z.len() > prefix.len() {
            return z[prefix.len()..].to_string();
        }
    }
    z.to_string()
}

fn listed(list: &[&str], zone: &str) -> bool {
    list.iter().any(|z| z.eq_ignore_ascii_case(zone))
}

/// Place a normalised zone. `None` means it fell through every list (the guard test asserts `Some` for
/// every zone chrono-tz knows, so a new one fails the build until a person classifies it).
fn classify(zone: &str, windows: bool) -> Option<Region> {
    if is_unplaceable(zone) {
        return Some(Region::Unplaceable { zone: zone.into() });
    }
    // Windows reports one golden id per Windows zone: "FLE Standard Time" (Finland, Estonia, Latvia,
    // Lithuania, Ukraine) is `Europe/Kiev`, so an EU laptop in Helsinki would read as outside. Those
    // ids can't be placed there.
    if windows && listed(AMBIGUOUS_ON_WINDOWS, zone) {
        return Some(Region::Unplaceable { zone: zone.into() });
    }
    if let Some((_, kind)) = IN_REGION.iter().find(|(z, _)| z.eq_ignore_ascii_case(zone)) {
        return Some(Region::Restricted {
            kind: *kind,
            zone: zone.into(),
        });
    }
    let area = zone.split('/').next().unwrap_or_default();
    if listed(OUTSIDE_IDS, zone) || (zone.contains('/') && listed(OUTSIDE_AREAS, area)) {
        return Some(Region::Outside);
    }
    None
}

/// Zones that say nothing about where a computer is.
fn is_unplaceable(zone: &str) -> bool {
    let z = zone.to_ascii_lowercase();
    z.is_empty()
        || z.starts_with("etc/")
        || matches!(
            z.as_str(),
            "utc" | "gmt" | "gmt0" | "gmt+0" | "gmt-0" | "greenwich" | "uct" | "universal" | "zulu"
        )
}

/// Ids PM reads as inside an area, matched on the id string and never canonicalised, because many are
/// tzdata links (Oslo and Stockholm link to Berlin) that must each be listed. 58 ids from tzdata 2026f
/// plus the Crown dependencies and Gibraltar (UK), Svalbard and Jan Mayen (EEA), the EU's outermost
/// regions that have non-European ids (GDPR applies there) and the legacy European names.
const IN_REGION: &[(&str, RegionKind)] = &[
    // The United Kingdom, its Crown dependencies and Gibraltar (conservative: over-blocking costs a
    // paid-key paste in a few tiny places).
    ("Europe/London", RegionKind::Uk),
    ("Europe/Belfast", RegionKind::Uk),
    ("GB", RegionKind::Uk),
    ("GB-Eire", RegionKind::Uk),
    ("Europe/Jersey", RegionKind::Uk),
    ("Europe/Guernsey", RegionKind::Uk),
    ("Europe/Isle_of_Man", RegionKind::Uk),
    ("Europe/Gibraltar", RegionKind::Uk),
    // Switzerland.
    ("Europe/Zurich", RegionKind::Switzerland),
    // The 27 EU states.
    ("Europe/Vienna", RegionKind::Eea),
    ("Europe/Brussels", RegionKind::Eea),
    ("Europe/Sofia", RegionKind::Eea),
    ("Europe/Zagreb", RegionKind::Eea),
    ("Asia/Nicosia", RegionKind::Eea),
    ("Asia/Famagusta", RegionKind::Eea),
    ("Europe/Nicosia", RegionKind::Eea),
    ("Europe/Prague", RegionKind::Eea),
    ("Europe/Copenhagen", RegionKind::Eea),
    ("Europe/Tallinn", RegionKind::Eea),
    ("Europe/Helsinki", RegionKind::Eea),
    ("Europe/Mariehamn", RegionKind::Eea),
    ("Europe/Paris", RegionKind::Eea),
    ("Europe/Berlin", RegionKind::Eea),
    ("Europe/Busingen", RegionKind::Eea),
    ("Europe/Athens", RegionKind::Eea),
    ("Europe/Budapest", RegionKind::Eea),
    ("Europe/Dublin", RegionKind::Eea),
    ("Eire", RegionKind::Eea),
    ("Europe/Rome", RegionKind::Eea),
    ("Europe/Riga", RegionKind::Eea),
    ("Europe/Vilnius", RegionKind::Eea),
    ("Europe/Luxembourg", RegionKind::Eea),
    ("Europe/Malta", RegionKind::Eea),
    ("Europe/Amsterdam", RegionKind::Eea),
    ("Europe/Warsaw", RegionKind::Eea),
    ("Poland", RegionKind::Eea),
    ("Europe/Lisbon", RegionKind::Eea),
    ("Portugal", RegionKind::Eea),
    ("Atlantic/Azores", RegionKind::Eea),
    ("Atlantic/Madeira", RegionKind::Eea),
    ("Europe/Bucharest", RegionKind::Eea),
    ("Europe/Bratislava", RegionKind::Eea),
    ("Europe/Ljubljana", RegionKind::Eea),
    ("Europe/Madrid", RegionKind::Eea),
    ("Africa/Ceuta", RegionKind::Eea),
    ("Atlantic/Canary", RegionKind::Eea),
    ("Europe/Stockholm", RegionKind::Eea),
    // Iceland, Liechtenstein, Norway; Svalbard and Jan Mayen (about no users, and it avoids a hole
    // through a Norwegian id).
    ("Atlantic/Reykjavik", RegionKind::Eea),
    ("Iceland", RegionKind::Eea),
    ("Europe/Vaduz", RegionKind::Eea),
    ("Europe/Oslo", RegionKind::Eea),
    ("Arctic/Longyearbyen", RegionKind::Eea),
    ("Atlantic/Jan_Mayen", RegionKind::Eea),
    // EU outermost regions with their own non-European ids.
    ("America/Martinique", RegionKind::Eea),
    ("America/Guadeloupe", RegionKind::Eea),
    ("America/Marigot", RegionKind::Eea),
    ("America/Cayenne", RegionKind::Eea),
    ("Indian/Reunion", RegionKind::Eea),
    ("Indian/Mayotte", RegionKind::Eea),
    // Legacy names that are links to European zones.
    ("CET", RegionKind::Eea),
    ("MET", RegionKind::Eea),
    ("EET", RegionKind::Eea),
    ("WET", RegionKind::Eea),
];

/// Ids positively known to be somewhere else: the European, Atlantic and Arctic ids that are not in
/// [`IN_REGION`] (on purpose: Faroe, Andorra, Monaco, San Marino, the Vatican, the Balkans outside the
/// EU, Turkey, Ukraine, Moldova, Belarus, Russia, Bermuda, Cape Verde, South Georgia, St Helena,
/// Stanley) and the legacy un-prefixed non-European names. A non-European AREA is in
/// [`OUTSIDE_AREAS`].
const OUTSIDE_IDS: &[&str] = &[
    "Atlantic/Bermuda",
    "Atlantic/Cape_Verde",
    "Atlantic/Faeroe",
    "Atlantic/Faroe",
    "Atlantic/South_Georgia",
    "Atlantic/St_Helena",
    "Atlantic/Stanley",
    "Europe/Andorra",
    "Europe/Astrakhan",
    "Europe/Belgrade",
    "Europe/Chisinau",
    "Europe/Istanbul",
    "Europe/Kaliningrad",
    "Europe/Kiev",
    "Europe/Kirov",
    "Europe/Kyiv",
    "Europe/Minsk",
    "Europe/Monaco",
    "Europe/Moscow",
    "Europe/Podgorica",
    "Europe/Samara",
    "Europe/San_Marino",
    "Europe/Sarajevo",
    "Europe/Saratov",
    "Europe/Simferopol",
    "Europe/Skopje",
    "Europe/Tirane",
    "Europe/Tiraspol",
    "Europe/Ulyanovsk",
    "Europe/Uzhgorod",
    "Europe/Vatican",
    "Europe/Volgograd",
    "Europe/Zaporozhye",
    "Turkey",
    "W-SU",
    "Japan",
    "NZ",
    "NZ-CHAT",
    "PRC",
    "ROC",
    "ROK",
    "Singapore",
    "Hongkong",
    "Israel",
    "Iran",
    "Egypt",
    "Libya",
    "Jamaica",
    "Cuba",
    "Navajo",
    "Kwajalein",
    "EST",
    "MST",
    "HST",
    "EST5EDT",
    "CST6CDT",
    "MST7MDT",
    "PST8PDT",
];

/// A first path segment that is not European: any id under it is outside (the IN list is checked
/// first, so `Africa/Ceuta` and `America/Martinique` stay in).
const OUTSIDE_AREAS: &[&str] = &[
    "America",
    "Asia",
    "Africa",
    "Australia",
    "Pacific",
    "Indian",
    "Antarctica",
    "US",
    "Canada",
    "Brazil",
    "Chile",
    "Mexico",
];

/// Ids a Windows machine reports for zones that contain both EU and non-EU places, so they can't be
/// read as "outside" (see [`classify`]).
const AMBIGUOUS_ON_WINDOWS: &[&str] = &[
    "Europe/Kiev",
    "Europe/Kyiv",
    "Europe/Uzhgorod",
    "Europe/Zaporozhye",
    "Europe/Chisinau",
    "Europe/Tiraspol",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn of(zone: &str) -> Region {
        region_of_on(Some(zone), false)
    }

    fn kind(zone: &str) -> Option<RegionKind> {
        match of(zone) {
            Region::Restricted { kind, .. } => Some(kind),
            _ => None,
        }
    }

    #[test]
    fn the_united_kingdom_switzerland_and_the_eea_are_restricted() {
        for z in [
            "Europe/London",
            "Europe/Jersey",
            "Europe/Gibraltar",
            "Europe/Guernsey",
            "Europe/Isle_of_Man",
            "GB",
        ] {
            assert_eq!(kind(z), Some(RegionKind::Uk), "{z}");
        }
        assert_eq!(kind("Europe/Zurich"), Some(RegionKind::Switzerland));
        for z in [
            "Europe/Amsterdam",
            "Atlantic/Canary",
            "Atlantic/Reykjavik",
            "Europe/Oslo",
            "Arctic/Longyearbyen",
            "America/Martinique",
            "America/Marigot",
            "Europe/Vaduz",
            "Europe/Helsinki",
            "Africa/Ceuta",
            "CET",
        ] {
            assert_eq!(kind(z), Some(RegionKind::Eea), "{z}");
        }
    }

    /// Linux hands back the link target as it is, and case is not a tzdata distinction PM relies on.
    #[test]
    fn a_posix_or_right_prefix_and_odd_case_still_place_a_zone() {
        for z in [
            "posix/Europe/London",
            "right/Europe/London",
            "eurOPE/lOndon",
            "  Europe/London\n",
        ] {
            assert_eq!(kind(z), Some(RegionKind::Uk), "{z:?}");
        }
        // The refusal names what PM read, minus the prefix.
        let Region::Restricted { zone, .. } = of("posix/Europe/London") else {
            panic!()
        };
        assert_eq!(zone, "Europe/London");
    }

    #[test]
    fn positively_known_places_elsewhere_are_outside() {
        for z in [
            "America/New_York",
            "Asia/Tokyo",
            "Europe/Moscow",
            "Atlantic/Faroe",
            "Europe/Kyiv",
            "Europe/Andorra",
            "Australia/Sydney",
            "Japan",
            "Turkey",
            "US/Pacific",
        ] {
            assert_eq!(of(z), Region::Outside, "{z}");
        }
    }

    #[test]
    fn utc_offsets_and_nothing_readable_are_unplaceable() {
        for z in [
            "UTC",
            "Etc/UTC",
            "Etc/GMT+3",
            "GMT",
            "Zulu",
            "Greenwich",
            "UCT",
            "Universal",
            "",
        ] {
            assert!(matches!(of(z), Region::Unplaceable { .. }), "{z:?}");
        }
        assert!(matches!(
            region_of_on(None, false),
            Region::Unplaceable { .. }
        ));
        assert!(
            of("UTC").refusal(Basis::Unknown).is_some()
                && Region::Outside.refusal(Basis::Unknown).is_none()
        );
    }

    /// FAIL CLOSED: an id no list knows is never read as outside. The first draft said "anything not
    /// listed is outside", which fails open on exactly what a newer system produces.
    #[test]
    fn an_id_no_list_knows_is_unplaceable_never_outside() {
        for z in [
            "Europe/Nowhere",
            "Atlantic/Newplace",
            "Arctic/Newplace",
            "Nowhere",
            "posix/Europe/Nowhere",
        ] {
            assert!(matches!(of(z), Region::Unplaceable { .. }), "{z}");
        }
        // And the zone is named for the refusal.
        let Region::Unplaceable { zone } = of("Europe/Nowhere") else {
            panic!()
        };
        assert_eq!(zone, "Europe/Nowhere");
    }

    /// The id comes off the OS, and every settings read places it, so an id with a multi-byte
    /// character where the `posix/` prefix would end must fall through to Unplaceable, not panic (the
    /// first cut sliced by bytes). Byte 6 of "aéééé" is inside an "é".
    #[test]
    fn a_zone_id_with_multibyte_characters_is_unplaceable_not_a_panic() {
        for z in [
            "aéééé",
            "poséx/Europe/London",
            "righé/x",
            "é",
            "日本語/東京",
            "posix/é",
        ] {
            assert!(matches!(of(z), Region::Unplaceable { .. }), "{z}");
        }
        // And it fails closed: a free key is refused on a zone PM can't place.
        assert!(region_of_on(Some("aéééé"), false)
            .refusal(Basis::Free)
            .is_some());
    }

    /// Windows' golden ids for FLE and E. Europe read as Ukraine and Moldova, which would put a
    /// Helsinki or Riga laptop outside. There they can't be placed instead; elsewhere they are outside.
    #[test]
    fn on_windows_the_ukraine_and_moldova_ids_are_unplaceable() {
        for z in ["Europe/Kiev", "Europe/Kyiv", "Europe/Chisinau"] {
            assert!(
                matches!(region_of_on(Some(z), true), Region::Unplaceable { .. }),
                "{z}"
            );
            assert_eq!(region_of_on(Some(z), false), Region::Outside, "{z}");
        }
        assert_eq!(
            region_of_on(Some("Europe/London"), true),
            region_of_on(Some("Europe/London"), false)
        );
    }

    /// Every zone chrono-tz knows must be placed by a list. A chrono-tz upgrade that adds or renames
    /// one fails here until a person classifies it, which is what makes a fixed list safe.
    fn unclassified<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
        names
            .filter(|n| classify(&normalise(n), false).is_none())
            .collect()
    }

    #[test]
    fn every_zone_chrono_tz_knows_is_classified() {
        let missing = unclassified(chrono_tz::TZ_VARIANTS.iter().map(|v| v.name()));
        assert!(
            missing.is_empty(),
            "classify these in region.rs: {missing:?}"
        );
    }

    #[test]
    fn the_guard_notices_a_zone_nobody_classified() {
        let fake = ["Europe/London", "Europe/Nowhere"];
        assert_eq!(unclassified(fake.into_iter()), vec!["Europe/Nowhere"]);
    }

    #[test]
    fn a_refusal_carries_the_label_the_zone_and_why() {
        let r = of("Europe/London").refusal(Basis::Free).unwrap();
        assert_eq!(
            (
                r.kind.as_str(),
                r.label.as_str(),
                r.zone.as_str(),
                r.basis.as_str()
            ),
            ("uk", "the United Kingdom", "Europe/London", "free")
        );
        let r = of("Etc/UTC").refusal(Basis::Unknown).unwrap();
        assert_eq!(
            (r.kind.as_str(), r.basis.as_str()),
            ("unplaceable", "unknown")
        );
        assert!(of("Asia/Tokyo").refusal(Basis::Free).is_none());
    }

    #[test]
    fn the_paused_sentence_names_the_place_and_the_zone() {
        let uk = of("Europe/London")
            .refusal(Basis::Free)
            .unwrap()
            .sentence(CloudProvider::Gemini);
        assert!(uk.starts_with("Gemini is paused:"), "{uk}");
        assert!(
            uk.contains("the United Kingdom") && uk.contains("Europe/London"),
            "{uk}"
        );
        assert!(
            uk.contains("Google says this key is on the free tier"),
            "{uk}"
        );
        let unknown = of("Europe/London")
            .refusal(Basis::Unknown)
            .unwrap()
            .sentence(CloudProvider::Gemini);
        assert!(
            unknown.contains("can't confirm this key is paid"),
            "{unknown}"
        );
        let nowhere = of("")
            .refusal(Basis::Free)
            .unwrap()
            .sentence(CloudProvider::Gemini);
        assert!(
            nowhere.contains("can't tell where this computer is")
                && nowhere.contains("reads as unknown"),
            "{nowhere}"
        );
    }

    /// The region comes from the OS and nothing else: no network and no stored webview zone.
    #[test]
    fn region_reads_no_network_and_no_stored_zone() {
        let src = include_str!("region.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();
        for word in [
            concat!("req", "west"),
            concat!("ht", "tp"),
            concat!("look", "up"),
            concat!("settings", "::"),
            concat!("get_", "setting"),
        ] {
            assert!(!production.contains(word), "region.rs mentions {word}");
        }
    }
}
