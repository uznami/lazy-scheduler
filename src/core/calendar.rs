use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduleItem {
    pub start: NaiveTime,
    pub duration: Duration,
    pub note: Option<String>,
}

#[derive(Debug)]
pub struct CalendarDay {
    pub work_start_time: Option<NaiveTime>,
    pub work_end_time: Option<NaiveTime>,
    pub scheduled_items: BTreeSet<ScheduleItem>,
}
impl CalendarDay {
    const EMPTY: &Self = &Self {
        work_start_time: None,
        work_end_time: None,
        scheduled_items: BTreeSet::new(),
    };
}

#[derive(Debug)]
pub struct Calendar {
    official_days: BTreeSet<NaiveDate>,
    working_time: (NaiveTime, NaiveTime),
    calendar_days: BTreeMap<NaiveDate, CalendarDay>,
}
impl Calendar {
    pub fn new(working_time: (NaiveTime, NaiveTime)) -> Self {
        Self {
            official_days: BTreeSet::new(),
            working_time,
            calendar_days: BTreeMap::new(),
        }
    }
    pub fn add_working_day(&mut self, date: NaiveDate, official: bool) {
        if official {
            self.official_days.insert(date);
        }
        self.calendar_days.insert(
            date,
            CalendarDay {
                work_start_time: None,
                work_end_time: None,
                scheduled_items: BTreeSet::new(),
            },
        );
    }
    pub fn remove_working_day(&mut self, date: NaiveDate, official: bool) {
        if official {
            self.official_days.remove(&date);
        }
        self.calendar_days.remove(&date);
    }
    pub fn add_scheduled_item(&mut self, date: &NaiveDate, item: ScheduleItem) -> bool {
        let Some(day) = self.calendar_days.get_mut(date) else {
            return false;
        };
        day.scheduled_items.insert(item);
        true
    }
    pub fn update_working_time(&mut self, date: NaiveDate, start: Option<NaiveTime>, end: Option<NaiveTime>) {
        let Some(day) = self.calendar_days.get_mut(&date) else {
            return;
        };
        if let (Some(start), Some(end)) = &(start, end) {
            if start >= end {
                return;
            }
        }
        day.work_start_time = start;
        day.work_end_time = end;
    }
    pub fn working_time(&self, date: NaiveDate) -> Option<(NaiveTime, NaiveTime)> {
        let day = self.calendar_days.get(&date)?;
        let start_time = day.work_start_time.unwrap_or(self.working_time.0);
        let end_time = day.work_end_time.unwrap_or(self.working_time.1);
        Some((start_time, end_time))
    }
    pub fn calendar_days(&self, start_date: &NaiveDate) -> impl Iterator<Item = (&NaiveDate, &CalendarDay)> {
        self.calendar_days.iter().skip_while(|(date, _)| *date < start_date)
    }
}

#[derive(Debug, Deserialize)]
struct WorkingTime {
    start: NaiveTime,
    end: NaiveTime,
}

#[derive(Debug, Deserialize)]
struct DateRange {
    start: NaiveDate,
    end: NaiveDate,
}

#[derive(Debug, Deserialize)]
struct Settings {
    default_working_time: WorkingTime,
    date_range: DateRange,
    holidays: Vec<NaiveDate>,
}

#[derive(Deserialize)]
struct OverridesConfig {
    override_holiday_to_workday: Vec<NaiveDate>,
    override_workday_to_holiday: Vec<NaiveDate>,
}

#[derive(Deserialize)]
struct DayScheduleConfig {
    start_time: Option<NaiveTime>,
    end_time: Option<NaiveTime>,
    schedule: Vec<DayScheduleItem>,
}
#[derive(Deserialize)]
struct DayScheduleItem {
    start: NaiveTime,
    end: NaiveTime,
    note: Option<String>,
}

pub enum TimeKind {
    Available,
    Busy(Box<Option<String>>),
}

/// ある日の時間の区間
pub struct TimeWindow {
    kind: TimeKind,
    pub date: NaiveDate,
    pub start: NaiveTime,
    pub end: NaiveTime,
}
impl TimeWindow {
    pub fn duration(&self) -> Duration {
        self.end.signed_duration_since(self.start)
    }
    pub fn start_datetime(&self) -> NaiveDateTime {
        self.date.and_time(self.start)
    }
    pub fn end_datetime(&self) -> NaiveDateTime {
        self.date.and_time(self.end)
    }
    pub fn available(&self) -> bool {
        matches!(self.kind, TimeKind::Available)
    }
    pub fn note(&self) -> &str {
        match &self.kind {
            TimeKind::Available => "",
            TimeKind::Busy(note) => note.as_ref().as_deref().unwrap_or(""),
        }
    }
}

impl Calendar {
    /// settings.yaml, override.yaml, schedule/*.yaml を読み込んで Calendar を構築
    pub fn import_from_yaml<P: AsRef<Path>>(settings_dirpath: P) -> Result<Self> {
        let settings_path = settings_dirpath.as_ref().join("settings.yaml");
        let overrides_path = settings_dirpath.as_ref().join("overrides.yaml");
        let schedule_dir = settings_dirpath.as_ref().join("schedule");

        // 1. 設定ファイル読み込み
        let s = fs::read_to_string(&settings_path).with_context(|| format!("failed to read {:?}", settings_path))?;
        let cfg: Settings = serde_yaml::from_str(&s).context("failed to parse settings.yaml")?;

        let od = if overrides_path.exists() {
            let o = fs::read_to_string(&overrides_path).with_context(|| format!("failed to read {:?}", overrides_path))?;
            serde_yaml::from_str(&o).context("failed to parse overrides.yaml")?
        } else {
            OverridesConfig {
                override_holiday_to_workday: Vec::new(),
                override_workday_to_holiday: Vec::new(),
            }
        };

        let mut cal = Calendar::new((cfg.default_working_time.start, cfg.default_working_time.end));

        let start = cfg.date_range.start;
        let end = cfg.date_range.end;
        let mut date = start;
        while date <= end {
            cal.add_working_day(date, true);
            date = date.succ_opt().unwrap();
        }

        // 4. holidays を休みに
        for h in cfg.holidays {
            cal.remove_working_day(h, true);
        }
        // overrides
        for w in od.override_holiday_to_workday {
            cal.add_working_day(w, false);
        }
        for h in od.override_workday_to_holiday {
            cal.remove_working_day(h, false);
        }

        // 5. schedule ディレクトリ内の *.yaml を読み込み
        for entry in fs::read_dir(schedule_dir)? {
            let path: PathBuf = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            // ファイル名から日付取得（例: "2023-10-01.yaml"）
            let fname = path.file_stem().unwrap().to_str().unwrap();
            let date = NaiveDate::parse_from_str(fname, "%Y-%m-%d")?;

            let txt = fs::read_to_string(&path)?;
            let day_cfg: DayScheduleConfig = serde_yaml::from_str(&txt).with_context(|| format!("failed to parse {:?}", path))?;

            // 日毎の就業時間を override
            cal.update_working_time(date, day_cfg.start_time, day_cfg.end_time);

            // schedule items
            for item in day_cfg.schedule {
                let start = item.start;
                let duration = item.end.signed_duration_since(item.start);
                let note = item.note;
                cal.add_scheduled_item(&date, ScheduleItem { start, duration, note });
            }
        }

        Ok(cal)
    }
    pub fn official_workdays(&self, start_at: NaiveDate) -> impl Iterator<Item = &NaiveDate> {
        self.official_days.iter().skip_while(move |date| *date < &start_at)
    }
    /// 指定の日付が全社公式稼働日か
    pub fn is_official_workday(&self, date: &NaiveDate) -> bool {
        self.official_days.contains(date)
    }
    /// 指定日より前の最後の公式稼働日
    pub fn previous_official_workday(&self, date: &NaiveDate) -> Option<NaiveDate> {
        self.official_days.range(..*date).cloned().next_back()
    }
    /// `from` 時点以降の公式稼働日について、時間ウィンドウを
    /// 日付順・時刻順に列挙するイテレータを返す
    pub fn time_windows(&self, from: NaiveDateTime) -> impl Iterator<Item = TimeWindow> {
        self.official_workdays(from.date()).flat_map(move |date| {
            // 1) 勤務時間帯を得る
            let (work_start, work_end) = self.working_time(*date).unwrap_or(self.working_time);
            // 2) 当日の予定済みアイテムを start 時刻順で取得
            let mut busy = self.calendar_days.get(date).map(|d| d.scheduled_items.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
            busy.sort_by_key(|item| item.start);
            // 3) 「from」と組み合わせて最初の window_start を決定
            let mut window_start = if *date == from.date() && from.time() > work_start { from.time() } else { work_start };
            // 4) 予定アイテム間のギャップを yield
            let mut windows = Vec::new();
            for item in busy {
                let item_start = item.start;
                if window_start < item_start {
                    windows.push(TimeWindow {
                        kind: TimeKind::Available,
                        date: *date,
                        start: window_start,
                        end: item_start,
                    });
                    windows.push(TimeWindow {
                        kind: TimeKind::Busy(Box::new(item.note)),
                        date: *date,
                        start: item_start,
                        end: item.start + item.duration,
                    });
                }
                // 次の窓はこのアイテムの end 時刻以降
                window_start = (item.start + item.duration).min(work_end);
            }
            // 5) 最後に勤務終了までのギャップ
            if window_start < work_end {
                windows.push(TimeWindow {
                    kind: TimeKind::Available,
                    date: *date,
                    start: window_start,
                    end: work_end,
                });
            }
            windows.into_iter()
        })
    }

    /// `until` までの公式稼働日について、時間ウィンドウを
    /// 日付順・時刻順に列挙するイテレータを逆順に返す (free_time_windows() の逆)
    pub fn time_windows_rev(&self, until: NaiveDateTime) -> impl Iterator<Item = TimeWindow> {
        self.official_days.range(..=until.date()).rev().flat_map(move |&date| {
            let (work_start, work_end) = self.working_time(date).unwrap_or(self.working_time);

            // 「until 日」の場合は時間も制限
            let mut window_end = if date == until.date() { std::cmp::min(work_end, until.time()) } else { work_end };

            let mut windows = Vec::new();

            // 逆順で busy アイテムを走査し、ギャップを順次プッシュ
            if let Some(day) = self.calendar_days.get(&date) {
                for item in day.scheduled_items.iter().rev() {
                    let item_end = (item.start + item.duration).min(window_end);
                    if item_end < window_end {
                        windows.push(TimeWindow {
                            kind: TimeKind::Busy(Box::new(item.note.clone())),
                            date,
                            start: item.start,
                            end: item.start + item.duration,
                        });
                        windows.push(TimeWindow {
                            kind: TimeKind::Available,
                            date,
                            start: item_end,
                            end: window_end,
                        });
                    }
                    window_end = std::cmp::max(item.start, work_start);
                }
            }

            // 最後に「勤務開始 ～ 最後の予定開始」のギャップ
            if work_start < window_end {
                windows.push(TimeWindow {
                    kind: TimeKind::Available,
                    date,
                    start: work_start,
                    end: window_end,
                });
            }

            windows.into_iter()
        })
    }
}

#[test]
fn test_import_calendar() {
    let cal = Calendar::import_from_yaml("settings").unwrap();
    println!("{:#?}", cal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};

    fn tupled(windows: impl Iterator<Item = TimeWindow>) -> Vec<(NaiveDateTime, NaiveDateTime)> {
        windows.map(|w| (w.start_datetime(), w.end_datetime())).collect()
    }

    #[test]
    fn test_empty_schedule_single_day() {
        // カレンダー初期化：09:00–17:00、5/1のみ公式稼働日
        let mut cal = Calendar::new((NaiveTime::from_hms_opt(9, 0, 0).unwrap(), NaiveTime::from_hms_opt(17, 0, 0).unwrap()));
        let d1 = NaiveDate::from_ymd_opt(2025, 5, 1).unwrap();
        cal.add_working_day(d1, true);

        let from = NaiveDateTime::new(d1, NaiveTime::from_hms_opt(9, 0, 0).unwrap());
        let until = NaiveDateTime::new(d1, NaiveTime::from_hms_opt(17, 0, 0).unwrap());

        // 予定無し → 09:00–17:00 が１つ
        let fw = tupled(cal.time_windows(from));
        assert_eq!(fw, vec![(from, until)]);

        // rev 版も逆順にすると同じ
        let fw_rev = tupled(cal.time_windows_rev(until));
        assert_eq!(fw.iter().rev().cloned().collect::<Vec<_>>(), fw_rev);
    }

    #[test]
    fn test_single_busy_item() {
        // 1日＋真ん中に 11:00–12:30 の予定
        let mut cal = Calendar::new((NaiveTime::from_hms_opt(9, 0, 0).unwrap(), NaiveTime::from_hms_opt(17, 0, 0).unwrap()));
        let d1 = NaiveDate::from_ymd_opt(2025, 5, 2).unwrap();
        cal.add_working_day(d1, true);
        cal.add_scheduled_item(
            &d1,
            ScheduleItem {
                start: NaiveTime::from_hms_opt(11, 0, 0).unwrap(),
                duration: Duration::minutes(90),
                note: None,
            },
        );

        let from = NaiveDateTime::new(d1, NaiveTime::from_hms_opt(9, 0, 0).unwrap());
        let until = NaiveDateTime::new(d1, NaiveTime::from_hms_opt(17, 0, 0).unwrap());

        // free: 09:00–11:00, 12:30–17:00
        let fw = tupled(cal.time_windows(from));
        let expected = vec![
            (from, NaiveDateTime::new(d1, NaiveTime::from_hms_opt(11, 0, 0).unwrap())),
            (NaiveDateTime::new(d1, NaiveTime::from_hms_opt(12, 30, 0).unwrap()), until),
        ];
        assert_eq!(fw, expected);

        // rev 版も逆順で同じ
        let fw_rev = tupled(cal.time_windows_rev(until));
        assert_eq!(fw.iter().rev().cloned().collect::<Vec<_>>(), fw_rev);
    }

    #[test]
    fn test_multi_day() {
        // 5/1・5/2 の２日間、いずれも公式稼働日、予定は無し
        let mut cal = Calendar::new((NaiveTime::from_hms_opt(8, 0, 0).unwrap(), NaiveTime::from_hms_opt(16, 0, 0).unwrap()));
        let d1 = NaiveDate::from_ymd_opt(2025, 5, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2025, 5, 2).unwrap();
        cal.add_working_day(d1, true);
        cal.add_working_day(d2, true);

        let from = NaiveDateTime::new(d1, NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        let until = NaiveDateTime::new(d2, NaiveTime::from_hms_opt(16, 0, 0).unwrap());

        // free:
        //   5/1 8–16
        //   5/2 8–16
        let mut fw = tupled(cal.time_windows(from));
        let mut expected = vec![
            (from, NaiveDateTime::new(d1, NaiveTime::from_hms_opt(16, 0, 0).unwrap())),
            (NaiveDateTime::new(d2, NaiveTime::from_hms_opt(8, 0, 0).unwrap()), until),
        ];
        assert_eq!(fw, expected);

        // rev 版は expected を逆順
        let fw_rev = tupled(cal.time_windows_rev(until));
        expected.reverse();
        assert_eq!(fw_rev, expected);
    }

    #[test]
    fn test_from_within_busy_item() {
        let mut cal = Calendar::new((NaiveTime::from_hms_opt(9, 0, 0).unwrap(), NaiveTime::from_hms_opt(18, 0, 0).unwrap()));
        let d = NaiveDate::from_ymd_opt(2025, 5, 3).unwrap();
        cal.add_working_day(d, true);
        cal.add_scheduled_item(
            &d,
            ScheduleItem {
                start: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
                duration: Duration::hours(2), // 10:00–12:00
                note: None,
            },
        );

        let from = NaiveDateTime::new(d, NaiveTime::from_hms_opt(11, 0, 0).unwrap());
        let until = NaiveDateTime::new(d, NaiveTime::from_hms_opt(11, 0, 0).unwrap());

        // from が busy の途中 (11:00) → 最初の free は 12:00–18:00
        let fw = tupled(cal.time_windows(from));
        let expected = vec![(
            NaiveDateTime::new(d, NaiveTime::from_hms_opt(12, 0, 0).unwrap()),
            NaiveDateTime::new(d, NaiveTime::from_hms_opt(18, 0, 0).unwrap()),
        )];
        assert_eq!(fw, expected);

        let fw_rev = tupled(cal.time_windows_rev(until));
        let expected = vec![(
            NaiveDateTime::new(d, NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
            NaiveDateTime::new(d, NaiveTime::from_hms_opt(10, 0, 0).unwrap()),
        )];
        assert_eq!(fw_rev, expected);
    }
}
