use chrono::NaiveDate;

use super::{
    slot::SlotMap,
    task::{self, Task, TaskID},
    work_log::{WorkLog, WorkLogItem},
};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufWriter, Write},
    path::{self, Path},
};

pub fn save_tasks<P: AsRef<Path>>(tasks: &BTreeMap<TaskID, Task>, path: P) -> anyhow::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    let tasks: Vec<_> = tasks.values().collect();
    serde_json::to_writer(&mut writer, &tasks)?;
    Ok(())
}

pub fn load_tasks<P: AsRef<Path>>(path: P) -> anyhow::Result<BTreeMap<TaskID, Task>> {
    if !path.as_ref().exists() {
        return Ok(BTreeMap::new()); // Return an empty vector if the file does not exist
    }
    let file = File::open(path)?;
    let tasks: Vec<Task> = serde_json::from_reader(file)?;
    let tasks = tasks.into_iter().map(|task| (task.id, task)).collect();
    Ok(tasks)
}

pub fn save_worklog<P: AsRef<Path>>(worklog: &WorkLog, path: P) -> anyhow::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &worklog.items())?;
    Ok(())
}

pub fn load_worklog<P: AsRef<Path>>(path: P) -> anyhow::Result<WorkLog> {
    if !path.as_ref().exists() {
        return Ok(WorkLog::new()); // Return an empty vector if the file does not exist
    }
    let file = File::open(path)?;
    let items: BTreeMap<NaiveDate, Vec<WorkLogItem>> = serde_json::from_reader(file)?;
    let worklog = WorkLog::from_items(items);
    Ok(worklog)
}
