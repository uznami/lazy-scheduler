use super::task::TaskID;
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug)]
pub struct SlotMap {
    slots: BTreeMap<NaiveDate, BTreeMap<TaskID, Duration>>,
    empty_slots: BTreeMap<TaskID, Duration>,
}
impl SlotMap {
    pub fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
            empty_slots: BTreeMap::new(),
        }
    }

    pub fn remaining_at(&self, date: &NaiveDate, task_id: TaskID) -> Option<Duration> {
        self.slots.get(date).and_then(|tasks| tasks.get(&task_id)).copied()
    }

    pub fn add(&mut self, date: NaiveDate, task_id: TaskID, duration: Duration) {
        let entry = self.slots.entry(date).or_default();
        if let Some(existing_duration) = entry.get_mut(&task_id) {
            *existing_duration += duration;
        } else {
            entry.insert(task_id, duration);
        }
    }

    pub fn consume(&mut self, date: &NaiveDate, task_id: TaskID, duration: Duration) {
        if let Some(tasks) = self.slots.get_mut(date) {
            if let Some(allocated) = tasks.get_mut(&task_id) {
                *allocated -= duration;
                if *allocated <= Duration::zero() {
                    tasks.remove(&task_id);
                }
            }
        }
    }

    pub fn get(&self, date: &NaiveDate) -> &BTreeMap<TaskID, Duration> {
        self.slots.get(date).unwrap_or(&self.empty_slots)
    }
}
