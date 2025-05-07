use super::{
    calendar::Calendar,
    deadline::Deadline,
    estimate::Estimate,
    schedule,
    slot::SlotMap,
    task::{ExternalBlockingReason, Progress, Task, TaskID},
    utils::StopKind,
    work_log::WorkLog,
};
use anyhow::bail;
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use core::task;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug)]
pub struct Session {
    pub calendar: Calendar,
    pub scheduler: schedule::Scheduler,
    pub tasks: BTreeMap<TaskID, Task>,
    pub slots: SlotMap,
    pub log: WorkLog,
    pub active_task: Option<(TaskID, NaiveDateTime)>,
    pub dirty_tasks: bool,
}
impl Session {
    pub fn new(calendar: Calendar, tasks: BTreeMap<TaskID, Task>, log: WorkLog) -> Self {
        let scheduler = schedule::Scheduler {
            work_tick: Duration::minutes(25),
            buffer_time: Duration::minutes(5),
            working_time: (NaiveTime::from_hms_opt(8, 45, 0).unwrap(), NaiveTime::from_hms_opt(19, 0, 0).unwrap()),
        };
        let mut slots = SlotMap::new();
        Self {
            calendar,
            scheduler,
            tasks,
            slots,
            log,
            active_task: None,
            dirty_tasks: false,
        }
    }
    pub fn add_task(&mut self, task: Task) -> &Task {
        let task_id = task.id;
        if self.tasks.contains_key(&task_id) {
            panic!("Task with ID {} already exists", task_id);
        }
        self.tasks.insert(task_id, task);
        self.dirty_tasks = true;
        self.tasks.get(&task_id).expect("Task not found")
    }
    pub fn iter_tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }
    pub fn find_task_by_prefix(&self, id_prefix: &str) -> Option<TaskID> {
        let found_keys = self.tasks.keys().filter(|id| id.starts_with(id_prefix)).cloned().collect::<Vec<_>>();
        match found_keys.len() {
            0 => None,
            1 => Some(found_keys[0]),
            _ => None,
        }
    }
    pub fn drop_task(&mut self, task_id: &TaskID) -> String {
        let mut task = self.tasks.get_mut(task_id).expect("Task not found");
        let task_title = task.title.clone();
        task.drop();
        self.dirty_tasks = true;
        task_title
    }
    pub fn set_deadline(&mut self, task_id: &TaskID, deadline: Deadline) -> &Task {
        let task = self.tasks.get_mut(task_id).expect("Task not found");
        task.deadline = deadline;
        self.dirty_tasks = true;
        task
    }
    pub fn estimate_task(&mut self, task_id: &TaskID, estimate: Estimate) -> anyhow::Result<&Task> {
        let mut task = self.tasks.get_mut(task_id).expect("Task not found");
        task.update_remaining(estimate).map_err(anyhow::Error::msg)?;
        self.dirty_tasks = true;
        Ok(task)
    }
    pub fn update_progress_task(&mut self, task_id: &TaskID, progress: Option<Progress>) -> &Task {
        let mut task = self.tasks.get_mut(task_id).expect("Task not found");
        task.progress = progress;
        self.dirty_tasks = true;
        task
    }
    pub fn schedule(&mut self, now: NaiveDateTime) -> anyhow::Result<()> {
        self.slots = self.scheduler.schedule(now, &self.tasks, &self.calendar)?;
        Ok(())
    }
    pub fn start_task_at(&mut self, task_id: &TaskID, start_at: NaiveDateTime) -> (&Task, Duration) {
        let task = self.tasks.get(task_id).expect("Task not found");
        self.active_task = Some((task.id, start_at));
        self.dirty_tasks = true;
        let remaining = self.slots.remaining_at(&start_at.date(), *task_id).unwrap_or_else(|| task.remaining());
        (task, remaining.min(self.scheduler.work_tick))
    }
    pub fn complete_task(&mut self, task_id: &TaskID, completed_at: NaiveDateTime, duration: Option<Duration>) -> &Task {
        let task = self.tasks.get_mut(task_id).expect("Task not found");
        if let Some(duration) = duration {
            task.record(duration);
        }
        task.complete(completed_at);
        self.active_task = None;
        self.dirty_tasks = true;
        task
    }
    pub fn stop_current_task(&mut self, kind: StopKind, complete: bool) -> anyhow::Result<&Task> {
        let Some((task_id, start_at)) = self.active_task else {
            bail!("No active task to stop");
        };
        let task = self.tasks.get_mut(&task_id).expect("Task not found");
        match kind {
            StopKind::Immediately(now) => {
                if complete {
                    task.complete(now);
                }
            }
            StopKind::EndsAt(end_time) => {
                if start_at.date() != end_time.date() {
                    bail!("Cannot stop task at a different date.");
                }
                assert!(end_time >= start_at, "End time must be after start time");
                let duration = end_time - start_at;
                self.log.add_item(start_at.date(), task_id, start_at.time(), duration);
                self.slots.consume(&start_at.date(), task_id, duration);
                task.record(duration);
                if complete {
                    task.complete(end_time);
                }
            }
            StopKind::EndsIn(duration) => {
                let end_time = start_at + duration;
                self.log.add_item(start_at.date(), task_id, start_at.time(), duration);
                self.slots.consume(&start_at.date(), task_id, duration);
                task.record(duration);
                if complete {
                    task.complete(end_time);
                }
            }
        }
        self.active_task = None;
        self.dirty_tasks = true;
        Ok(task)
    }

    pub fn record_task(&mut self, task_id: &TaskID, duration: Duration) -> &Task {
        let task = self.tasks.get_mut(task_id).expect("Task not found");
        task.record(duration);
        self.dirty_tasks = true;
        task
    }

    pub fn block_task_by_tasks(&mut self, task_id: &TaskID, dependencies: Vec<TaskID>) -> (&Task, Vec<&Task>) {
        let task = self.tasks.get_mut(task_id).expect("Task not found");
        task.block_by_task(dependencies.clone());
        self.dirty_tasks = true;
        let task = self.tasks.get(task_id).expect("Task not found");
        let dependencies: Vec<_> = dependencies.iter().filter_map(|id| self.tasks.get(id)).collect();
        (task, dependencies)
    }

    pub fn block_task_by_external(&mut self, task_id: &TaskID, now: NaiveDateTime, until: Deadline, note: Option<String>) -> &Task {
        let task = self.tasks.get_mut(task_id).expect("Task not found");
        let reason = ExternalBlockingReason {
            may_unblock_at: until,
            note,
            last_updated: now,
        };
        task.block_by_external(reason);
        self.dirty_tasks = true;
        task
    }
}
