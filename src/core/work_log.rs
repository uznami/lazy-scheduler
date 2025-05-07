use std::collections::BTreeMap;

use super::task::{self, TaskID};
use chrono::{Duration, NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkLogItem {
    pub begin_at: NaiveTime,
    pub duration: Duration,
    pub task_id: TaskID,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkLog {
    dirty: bool,
    items: BTreeMap<NaiveDate, Vec<WorkLogItem>>,
}
impl WorkLog {
    pub fn new() -> Self {
        Self { dirty: false, items: BTreeMap::new() }
    }
    pub fn from_items(items: BTreeMap<NaiveDate, Vec<WorkLogItem>>) -> Self {
        Self { dirty: false, items }
    }

    pub fn add_item(&mut self, date: NaiveDate, task_id: TaskID, begin_at: NaiveTime, duration: Duration) {
        let item = WorkLogItem { begin_at, duration, task_id };
        self.items.entry(date).or_default().push(item);
        self.dirty = true;
    }

    pub fn get_items(&self, date: NaiveDate) -> Option<&Vec<WorkLogItem>> {
        self.items.get(&date)
    }

    pub fn total_recorded_duration(&self, task_id: TaskID) -> Duration {
        self.items
            .values()
            .flat_map(|items| items.iter())
            .filter(|item| item.task_id == task_id)
            .map(|item| item.duration)
            .sum()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn items(&self) -> &BTreeMap<NaiveDate, Vec<WorkLogItem>> {
        &self.items
    }
}
