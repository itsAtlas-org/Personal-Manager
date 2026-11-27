// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dates a person can hold with parts missing: a birthday with no year, "met in 2019", a school
//! that ran "2008-09" to "2012". The stored forms are `YYYY-MM-DD`, `--MM-DD` (no year), `YYYY-MM`
//! and `YYYY`, and this module is the one place that parses, validates and writes them.
//!
//! A year-less 29 February is valid (someone born on it has a birthday every year): the day is
//! checked against a leap year. Pure: no clock, no database.
//!
//! A 29 February birthday keeps one more fact, which day to mark in years without one
//! ([`kept_leap_day`], and `born_on_in` for the tests; spec E22).

use super::LeapDay;

/// One parsed date. At least a year, or a month and a day, is always present.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PartialDate {
    pub year: Option<u16>,
    pub month: Option<u8>,
    pub day: Option<u8>,
}

/// The year used only to check a year-less day: a leap one, so 29 February passes.
const LEAP_PLACEHOLDER: u16 = 2000;

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap =
                (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
            if leap {
                29
            } else {
                28
            }
        }
    }
}

/// Exactly `n` ASCII digits, as a number.
fn digits(text: &str, n: usize) -> Option<u32> {
    if text.len() != n || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl PartialDate {
    /// Parse `YYYY`, `YYYY-MM`, `YYYY-MM-DD` or `--MM-DD`, refusing anything that is not a real
    /// date (a 13th month, 31 April, 29 February of a common year). Year 0000 is refused: no
    /// calendar app takes it, and it is how a missing year is often mis-written.
    pub fn parse(text: &str) -> Option<PartialDate> {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix("--") {
            let (m, d) = rest.split_once('-')?;
            let month = u8::try_from(digits(m, 2)?).ok()?;
            let day = u8::try_from(digits(d, 2)?).ok()?;
            if !(1..=12).contains(&month)
                || day == 0
                || day > days_in_month(LEAP_PLACEHOLDER, month)
            {
                return None;
            }
            return Some(PartialDate {
                year: None,
                month: Some(month),
                day: Some(day),
            });
        }
        let mut it = text.splitn(3, '-');
        let year = u16::try_from(digits(it.next()?, 4)?).ok()?;
        if year == 0 {
            return None;
        }
        let Some(m) = it.next() else {
            return Some(PartialDate {
                year: Some(year),
                month: None,
                day: None,
            });
        };
        let month = u8::try_from(digits(m, 2)?).ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        let Some(d) = it.next() else {
            return Some(PartialDate {
                year: Some(year),
                month: Some(month),
                day: None,
            });
        };
        let day = u8::try_from(digits(d, 2)?).ok()?;
        if day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(PartialDate {
            year: Some(year),
            month: Some(month),
            day: Some(day),
        })
    }

    /// Whether this is a full day (a month and a day), with or without a year: the shape a
    /// birthday takes.
    pub fn is_day(&self) -> bool {
        self.month.is_some() && self.day.is_some()
    }

    /// The stored text: `YYYY-MM-DD`, `--MM-DD`, `YYYY-MM` or `YYYY`.
    pub fn to_iso(self) -> String {
        match (self.year, self.month, self.day) {
            (Some(y), Some(m), Some(d)) => format!("{y:04}-{m:02}-{d:02}"),
            (None, Some(m), Some(d)) => format!("--{m:02}-{d:02}"),
            (Some(y), Some(m), None) => format!("{y:04}-{m:02}"),
            (Some(y), _, _) => format!("{y:04}"),
            // Unreachable for a parsed value; kept total so a hand-built one never panics.
            _ => String::new(),
        }
    }

    /// How PM shows it: `DD-MM-YYYY`, `MM-YYYY`, `YYYY`, or "15 April" when the year is unknown.
    /// The page formats dates itself (`format.ts`); this is the Rust twin the tests hold it to.
    #[cfg(test)]
    pub fn to_display(self) -> String {
        const MONTHS: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        match (self.year, self.month, self.day) {
            (Some(y), Some(m), Some(d)) => format!("{d:02}-{m:02}-{y:04}"),
            (None, Some(m), Some(d)) => format!("{d} {}", MONTHS[usize::from(m) - 1]),
            (Some(y), Some(m), None) => format!("{m:02}-{y:04}"),
            (Some(y), _, _) => format!("{y:04}"),
            _ => String::new(),
        }
    }
}

/// A birthday as stored: a full day, with or without a year. Returns the canonical text.
pub fn normalise_born_on(text: &str) -> Option<String> {
    PartialDate::parse(text)
        .filter(PartialDate::is_day)
        .map(|d| d.to_iso())
}

/// Whether a stored birthday is 29 February, with or without a year.
pub fn is_leap_day_born_on(born_on: &str) -> bool {
    PartialDate::parse(born_on).is_some_and(|d| d.month == Some(2) && d.day == Some(29))
}

/// The 29 February answer to store beside `born_on` (E22): the answer itself while the birthday is
/// 29 February, nothing otherwise. Every write path runs its birthday and answer through this, so
/// a changed or cleared date never leaves a stale answer behind.
pub fn kept_leap_day(born_on: Option<&str>, answer: Option<LeapDay>) -> Option<LeapDay> {
    answer.filter(|_| born_on.is_some_and(is_leap_day_born_on))
}

/// The civil date a birthday falls on in `year`, as month and day. A 29 February one lands on
/// itself in a leap year, and in other years on the day the person chose: 28 February, or 1 March.
/// Unanswered reads as 28 February. `None` when `born_on` is not a full day. The one owner of this
/// rule on the Rust side; `partialDate.ts` holds its twin. Nothing in PR A calls it (the page does
/// this sum); it is here for the tests to pin the rule against the TS twin.
#[cfg(test)]
pub fn born_on_in(born_on: &str, answer: Option<LeapDay>, year: u16) -> Option<PartialDate> {
    let d = PartialDate::parse(born_on).filter(PartialDate::is_day)?;
    let (month, day) = (d.month?, d.day?);
    let (month, day) = if (month, day) == (2, 29) && days_in_month(year, 2) == 28 {
        match answer {
            Some(LeapDay::Mar1) => (3, 1),
            _ => (2, 28),
        }
    } else {
        (month, day)
    };
    Some(PartialDate {
        year: Some(year),
        month: Some(month),
        day: Some(day),
    })
}

/// A date that may be partial from the front (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`), as `met_on` and a
/// detail's from and to dates are. A year-less day is not one of these.
pub fn normalise_dated(text: &str) -> Option<String> {
    PartialDate::parse(text)
        .filter(|d| d.year.is_some())
        .map(|d| d.to_iso())
}

/// Any of the four shapes, for a "date" detail (an anniversary may have no year).
pub fn normalise_any(text: &str) -> Option<String> {
    PartialDate::parse(text).map(|d| d.to_iso())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_round_trips() {
        for s in ["2001-04-15", "--04-15", "2001-04", "2001", "--02-29"] {
            assert_eq!(PartialDate::parse(s).unwrap().to_iso(), s);
        }
    }

    #[test]
    fn real_dates_only() {
        for s in [
            "2023-02-29",
            "--02-30",
            "--13-01",
            "2001-04-31",
            "2001-00",
            "0000",
            "01-04-2001",
            "2001-4-5",
            "",
            "--",
            "--04",
        ] {
            assert!(PartialDate::parse(s).is_none(), "{s:?}");
        }
        assert!(PartialDate::parse("2024-02-29").is_some());
        assert!(PartialDate::parse("2000-02-29").is_some());
        assert!(PartialDate::parse("1900-02-29").is_none());
    }

    #[test]
    fn each_field_takes_only_its_shapes() {
        assert_eq!(normalise_born_on("--02-29").as_deref(), Some("--02-29"));
        assert!(normalise_born_on("2001-04").is_none());
        assert!(normalise_dated("--04-15").is_none());
        assert_eq!(normalise_dated("2001-04").as_deref(), Some("2001-04"));
        assert_eq!(normalise_any("--04-15").as_deref(), Some("--04-15"));
    }

    fn lands(born: &str, answer: Option<LeapDay>, year: u16) -> (u8, u8) {
        let d = born_on_in(born, answer, year).unwrap();
        assert_eq!(d.year, Some(year));
        (d.month.unwrap(), d.day.unwrap())
    }

    #[test]
    fn a_29_february_birthday_lands_on_the_chosen_day_in_common_years() {
        for born in ["--02-29", "2000-02-29"] {
            // A common year: the chosen day, and 28 February while unanswered.
            assert_eq!(lands(born, Some(LeapDay::Feb28), 2027), (2, 28));
            assert_eq!(lands(born, Some(LeapDay::Mar1), 2027), (3, 1));
            assert_eq!(lands(born, None, 2027), (2, 28));
            // A leap year: itself, whatever the answer.
            for answer in [Some(LeapDay::Feb28), Some(LeapDay::Mar1), None] {
                assert_eq!(lands(born, answer, 2028), (2, 29));
            }
            // A century year is common unless divisible by 400.
            assert_eq!(lands(born, Some(LeapDay::Mar1), 2100), (3, 1));
            assert_eq!(lands(born, Some(LeapDay::Mar1), 2400), (2, 29));
        }
    }

    #[test]
    fn any_other_birthday_ignores_the_answer_and_a_partial_date_has_no_day() {
        assert_eq!(lands("--04-15", Some(LeapDay::Mar1), 2027), (4, 15));
        assert_eq!(lands("1990-02-28", Some(LeapDay::Mar1), 2027), (2, 28));
        assert!(born_on_in("1990-04", None, 2027).is_none());
        assert!(born_on_in("not a date", None, 2027).is_none());
    }

    #[test]
    fn the_answer_is_kept_only_for_a_29_february_birthday() {
        let mar1 = Some(LeapDay::Mar1);
        assert_eq!(kept_leap_day(Some("--02-29"), mar1), mar1);
        assert_eq!(kept_leap_day(Some("2000-02-29"), mar1), mar1);
        assert_eq!(kept_leap_day(Some("--03-01"), mar1), None);
        assert_eq!(kept_leap_day(Some("2000-02-28"), mar1), None);
        assert_eq!(kept_leap_day(None, mar1), None);
        assert_eq!(kept_leap_day(Some("--02-29"), None), None);
    }

    #[test]
    fn display_follows_the_house_format() {
        assert_eq!(
            PartialDate::parse("2001-04-05").unwrap().to_display(),
            "05-04-2001"
        );
        assert_eq!(
            PartialDate::parse("--04-15").unwrap().to_display(),
            "15 April"
        );
        assert_eq!(PartialDate::parse("2001").unwrap().to_display(), "2001");
    }
}
