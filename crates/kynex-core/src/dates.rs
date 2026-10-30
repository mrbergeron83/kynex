//! Sessions under date headers in the sessions panel: Today, Yesterday,
//! Previous 7 days, Previous 30 days, then a header per month for the last
//! year and per year before that. Sessions are filed by their last activity.
//! The rules work on calendar days, so they test without a clock or time zone;
//! [`local_day`] turns a file time into one.

use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local, NaiveDate};

/// A calendar day.
pub type Day = NaiveDate;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bucket {
    Today,
    Yesterday,
    Week,
    Month,
    /// A calendar month in the last year: (year, month 1–12).
    MonthOf(i32, u32),
    Year(i32),
}

impl Bucket {
    /// Where a session last active on `day` goes, seen on `today`.
    pub fn of(day: NaiveDate, today: NaiveDate) -> Self {
        match (today - day).num_days() {
            // A clock that ran ahead still counts as today.
            ..=0 => Bucket::Today,
            1 => Bucket::Yesterday,
            2..=7 => Bucket::Week,
            8..=30 => Bucket::Month,
            _ => {
                let months = (today.year() - day.year()) * 12 + today.month() as i32 - day.month() as i32;
                if months < 12 { Bucket::MonthOf(day.year(), day.month()) } else { Bucket::Year(day.year()) }
            }
        }
    }

    /// Its header: "Today", "October", "December 2025", "2024".
    pub fn label(self, today: NaiveDate) -> String {
        match self {
            Bucket::Today => "Today".into(),
            Bucket::Yesterday => "Yesterday".into(),
            Bucket::Week => "Previous 7 days".into(),
            Bucket::Month => "Previous 30 days".into(),
            Bucket::MonthOf(year, month) => {
                let name = MONTHS[(month as usize).clamp(1, 12) - 1];
                if year == today.year() { name.into() } else { format!("{name} {year}") }
            }
            Bucket::Year(year) => year.to_string(),
        }
    }

    /// A name for remembering whether it's folded: the same header keeps it
    /// from one day to the next.
    pub fn key(self) -> String {
        match self {
            Bucket::Today => "today".into(),
            Bucket::Yesterday => "yesterday".into(),
            Bucket::Week => "week".into(),
            Bucket::Month => "month".into(),
            Bucket::MonthOf(year, month) => format!("{year}-{month:02}"),
            Bucket::Year(year) => year.to_string(),
        }
    }

    /// The recent ones start open; months and years start folded.
    pub fn open_by_default(self) -> bool {
        matches!(self, Bucket::Today | Bucket::Yesterday | Bucket::Week | Bucket::Month)
    }
}

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

/// The local calendar day of a file time.
pub fn local_day(t: SystemTime) -> NaiveDate {
    DateTime::<Local>::from(t).date_naive()
}

/// Today, locally.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// `items` (newest first, each with the day it was last active) under their
/// headers, in order; empty buckets don't appear. One bucket only means no
/// headers are needed: then it's `None`.
pub fn sections<T: Clone>(items: &[(T, NaiveDate)], today: NaiveDate) -> Option<Vec<(Bucket, Vec<T>)>> {
    let mut out: Vec<(Bucket, Vec<T>)> = Vec::new();
    for (item, day) in items {
        let bucket = Bucket::of(*day, today);
        match out.iter_mut().find(|(b, _)| *b == bucket) {
            Some((_, list)) => list.push(item.clone()),
            None => out.push((bucket, vec![item.clone()])),
        }
    }
    (out.len() > 1).then_some(out)
}

/// [`sections`] for a list that falls on a single day's bucket: that one
/// section, for when other sections sit beside it and it needs its header too.
/// `None` when the items span several buckets (use [`sections`]) or none.
pub fn lone_section<T: Clone>(items: &[(T, NaiveDate)], today: NaiveDate) -> Option<Vec<(Bucket, Vec<T>)>> {
    let bucket = Bucket::of(items.first()?.1, today);
    items
        .iter()
        .all(|(_, day)| Bucket::of(*day, today) == bucket)
        .then(|| vec![(bucket, items.iter().map(|(item, _)| item.clone()).collect())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_bucket_can_still_have_its_header() {
        let today = d(2026, 10, 30);
        let items = [("a", today), ("b", today)];
        assert!(sections(&items, today).is_none());
        assert_eq!(lone_section(&items, today), Some(vec![(Bucket::Today, vec!["a", "b"])]));
        assert!(lone_section(&[("a", today), ("b", d(2026, 1, 1))], today).is_none());
        assert!(lone_section::<&str>(&[], today).is_none());
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn days_go_to_their_buckets() {
        let today = d(2026, 10, 28);
        assert_eq!(Bucket::of(d(2026, 10, 28), today), Bucket::Today);
        assert_eq!(Bucket::of(d(2026, 10, 29), today), Bucket::Today, "a clock ahead is still today");
        assert_eq!(Bucket::of(d(2026, 10, 27), today), Bucket::Yesterday);
        assert_eq!(Bucket::of(d(2026, 10, 21), today), Bucket::Week);
        assert_eq!(Bucket::of(d(2026, 10, 20), today), Bucket::Month);
        assert_eq!(Bucket::of(d(2026, 9, 28), today), Bucket::Month);
        assert_eq!(Bucket::of(d(2026, 9, 27), today), Bucket::MonthOf(2026, 9));
        assert_eq!(Bucket::of(d(2025, 11, 3), today), Bucket::MonthOf(2025, 11));
        assert_eq!(Bucket::of(d(2025, 10, 31), today), Bucket::Year(2025));
        assert_eq!(Bucket::of(d(2023, 1, 1), today), Bucket::Year(2023));
    }

    #[test]
    fn midnight_and_new_year() {
        // Just after midnight on New Year's Day, the 31st is yesterday.
        let today = d(2027, 1, 1);
        assert_eq!(Bucket::of(d(2026, 12, 31), today), Bucket::Yesterday);
        assert_eq!(Bucket::of(d(2026, 11, 15), today), Bucket::MonthOf(2026, 11));
        assert_eq!(Bucket::MonthOf(2026, 11).label(today), "November 2026");
        assert_eq!(Bucket::MonthOf(2027, 1).label(d(2027, 3, 1)), "January");
    }

    #[test]
    fn labels_and_keys() {
        let today = d(2026, 10, 28);
        assert_eq!(Bucket::Week.label(today), "Previous 7 days");
        assert_eq!(Bucket::Year(2024).label(today), "2024");
        assert_eq!(Bucket::MonthOf(2026, 3).key(), "2026-03");
        assert!(Bucket::Today.open_by_default() && !Bucket::MonthOf(2026, 3).open_by_default());
    }

    #[test]
    fn sections_keep_order_and_skip_a_lone_bucket() {
        let today = d(2026, 10, 28);
        let items = [("a", today), ("b", today), ("c", d(2026, 10, 27)), ("d", d(2026, 6, 2))];
        let s = sections(&items, today).unwrap();
        let shape: Vec<_> = s.iter().map(|(b, ids)| (*b, ids.clone())).collect();
        assert_eq!(
            shape,
            vec![
                (Bucket::Today, vec!["a", "b"]),
                (Bucket::Yesterday, vec!["c"]),
                (Bucket::MonthOf(2026, 6), vec!["d"])
            ]
        );
        assert_eq!(sections(&items[..2], today), None, "one bucket needs no header");
    }
}
