use super::calendar::Calendar;
use chrono::{Datelike, Duration, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FuzzyDeadlineKind {
    /// Due after n business days from the reference date.
    /// (e.g. 2025-04-01 + BusinessDays(2) = 2025-04-03)
    BusinessDays(u16),
    /// Due at the end of the weekday after n weeks from the reference date
    /// (e.g. 2025-04-30 + FridayOfWeeks(3) = friday_of_week(2025-04-30) + 3 * 7 days = 2025-05-23
    FridayOfWeeks(u16),
    /// Due after n weeks (n * 7 days) from the reference date
    /// (e.g. 2025-04-30 + Week(2) = 2025-04-30 + 2 * 7 days = 2025-05-14)
    Weeks(u16),
    /// Due at the end of the month after n months from the reference date
    /// (e.g. 2025-04-16 + MonthEnds(2) = 2025-06-30)
    MonthEnds(u16),
    /// Due after n months from the reference date
    /// (e.g. 2025-04-16 + Month(2) = 2025-06-16)
    Months(u16),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzyDeadline {
    /// The reference date for the fuzzy deadline.
    pub reference_date: NaiveDateTime,

    /// The kind of fuzzy deadline.
    pub kind: FuzzyDeadlineKind,

    /// The time of the deadline.
    pub time: Option<NaiveTime>,
}

impl FuzzyDeadline {
    pub fn new(reference_date: NaiveDateTime, kind: FuzzyDeadlineKind, time: Option<NaiveTime>) -> Self {
        Self { reference_date, kind, time }
    }
    pub fn resolve(&self, default_deadline_time: NaiveTime) -> NaiveDateTime {
        let base_date = self.reference_date.date();
        let deadline_date = match self.kind {
            FuzzyDeadlineKind::BusinessDays(day) => base_date + Duration::days(day as i64),
            FuzzyDeadlineKind::FridayOfWeeks(week) => {
                let start_of_week = base_date.week(chrono::Weekday::Mon).first_day();
                let friday = start_of_week + Duration::days(4);
                let week = start_of_week + chrono::Duration::weeks(week as i64);
                week + (friday - start_of_week)
            }
            FuzzyDeadlineKind::Weeks(week) => base_date + chrono::Duration::weeks(week as i64),
            FuzzyDeadlineKind::MonthEnds(month) => {
                let start_of_month = base_date.with_day(1).expect("with_day"); // SAFETY: all of month have a first day
                let month = start_of_month.month();
                start_of_month.iter_days().take_while(|d| d.month() == month).last().expect("last")
            }
            FuzzyDeadlineKind::Months(month) => {
                let start_of_month = base_date.with_day(1).expect("with_day"); // SAFETY: all of month have a first day
                start_of_month + chrono::Duration::weeks(4 * month as i64)
            }
        };
        let time = self.time.unwrap_or(default_deadline_time);
        deadline_date.and_time(time)
    }
    pub fn resolve_with_calendar(&self, calendar: &Calendar, default_deadline_time: NaiveTime) -> Result<NaiveDateTime, String> {
        use FuzzyDeadlineKind::*;
        let base_date = self.reference_date.date();

        // 1) 生の暦日計算
        let mut deadline_date = match self.kind {
            BusinessDays(day) => calendar
                .official_workdays(base_date)
                .nth(day as usize)
                .cloned()
                .ok_or_else(|| format!("{}日目の稼働日が見つかりません", day))?,
            FridayOfWeeks(week) => {
                let start_of_week = base_date.week(chrono::Weekday::Mon).first_day();
                let friday = start_of_week + Duration::days(4);
                let week = start_of_week + chrono::Duration::weeks(week as i64);
                week + (friday - start_of_week)
            }
            Weeks(week) => base_date + chrono::Duration::weeks(week as i64),
            MonthEnds(month) => {
                let start_of_month = base_date.with_day(1).expect("with_day"); // SAFETY: all of month have a first day
                let month = start_of_month.month();
                start_of_month.iter_days().take_while(|d| d.month() == month).last().expect("last")
            }
            Months(month) => {
                let start_of_month = base_date.with_day(1).expect("with_day"); // SAFETY: all of month have a first day
                start_of_month + chrono::Duration::weeks(4 * month as i64)
            }
        };

        // 2) 公式稼働日でなければ、直前の公式稼働日に丸め込む
        if !calendar.is_official_workday(&deadline_date) {
            if let Some(prev) = calendar.previous_official_workday(&deadline_date) {
                deadline_date = prev;
            }
        }

        let time = self.time.unwrap_or(default_deadline_time);
        Ok(deadline_date.and_time(time))
    }
}
#[test]
fn test_resolve_fuzzy_deadline() {
    let default_deadline_time = NaiveTime::from_hms_opt(20, 00, 00).unwrap();

    // ByDay
    let reference_date = NaiveDateTime::from_str("2025-04-30T00:00:00").unwrap();
    let fuzzy_deadline = FuzzyDeadline::new(reference_date, FuzzyDeadlineKind::BusinessDays(0), Some(NaiveTime::from_hms_opt(17, 00, 00).unwrap()));
    let resolved_date = fuzzy_deadline.resolve(default_deadline_time);
    assert_eq!(resolved_date, NaiveDateTime::from_str("2025-04-30T17:00:00").unwrap());
    let fuzzy_deadline = FuzzyDeadline::new(reference_date, FuzzyDeadlineKind::BusinessDays(3), Some(NaiveTime::from_hms_opt(17, 00, 00).unwrap()));
    let resolved_date = fuzzy_deadline.resolve(default_deadline_time);
    assert_eq!(resolved_date, NaiveDateTime::from_str("2025-05-03T17:00:00").unwrap());

    // FridayOfWeeks(0)
    let fuzzy_deadline = FuzzyDeadline::new(reference_date, FuzzyDeadlineKind::FridayOfWeeks(0), None);
    let resolved_date = fuzzy_deadline.resolve(default_deadline_time);
    assert_eq!(resolved_date, NaiveDateTime::from_str("2025-05-02T23:59:59").unwrap());

    // Weeks(n)
    let fuzzy_deadline = FuzzyDeadline::new(reference_date, FuzzyDeadlineKind::Weeks(2), None);
    let resolved_date = fuzzy_deadline.resolve(default_deadline_time);
    assert_eq!(resolved_date, NaiveDateTime::from_str("2025-05-14T23:59:59").unwrap());
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Deadline {
    None,
    Unknown,
    Exact(NaiveDateTime),
    Fuzzy(FuzzyDeadline),
}

impl Deadline {
    pub fn resolve_with_calendar(&self, calendar: &Calendar, default_deadline_time: NaiveTime) -> Result<Option<NaiveDateTime>, String> {
        match self {
            Deadline::None => Ok(None),
            Deadline::Unknown => Ok(None),
            Deadline::Exact(deadline) => Ok(Some(*deadline)),
            Deadline::Fuzzy(fuzzy_deadline) => {
                let resolved = fuzzy_deadline.resolve_with_calendar(calendar, default_deadline_time)?;
                Ok(Some(resolved))
            }
        }
    }
}
