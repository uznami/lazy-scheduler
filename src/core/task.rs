use super::{deadline::Deadline, estimate::Estimate};
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskID(Uuid);
impl TaskID {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0
            .as_bytes()
            .iter()
            .flat_map(|b| format!("{:02x}", b).chars().collect::<Vec<_>>())
            .zip(prefix.chars())
            .all(|(b, c)| b == c)
    }
}
impl From<[u8; 16]> for TaskID {
    fn from(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}
impl std::fmt::Display for TaskID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let short_uuid = self.0.as_bytes()[..3].iter().map(|b| format!("{:02x}", b)).collect::<String>();
        write!(f, "#{}", short_uuid)
    }
}
impl std::fmt::Debug for TaskID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // use Display trait for better debugging
        write!(f, "{}", self)
    }
}

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Progress(u8);
impl Progress {
    pub fn new(progress: u8) -> Result<Self, String> {
        if progress > 100 {
            return Err("Progress must be between 0 and 100".to_string());
        }
        Ok(Self(progress))
    }
    pub fn zero() -> Self {
        Self(0)
    }
    pub fn full() -> Self {
        Self(100)
    }
}
impl TryFrom<u8> for Progress {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Progress> for u8 {
    fn from(progress: Progress) -> Self {
        progress.0
    }
}
impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:3}%", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskID,
    pub title: String,
    pub created_at: NaiveDateTime,
    pub deadline: Deadline,
    status: TaskStatus,
    pub note: Option<String>,
    estimate: Option<Estimate>,
    pub progress: Option<Progress>,
    pub actual_total: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalBlockingReason {
    pub note: Option<String>,
    pub may_unblock_at: Deadline,
    pub last_updated: NaiveDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockingStatus {
    pub tasks: Vec<TaskID>,
    pub externals: Vec<ExternalBlockingReason>,
}
impl BlockingStatus {
    pub fn by_task(task_ids: Vec<TaskID>) -> Self {
        Self { tasks: task_ids, externals: vec![] }
    }
    pub fn by_external(external_reason: ExternalBlockingReason) -> Self {
        Self {
            tasks: vec![],
            externals: vec![external_reason],
        }
    }
    pub fn block_by_task(&mut self, task_ids: Vec<TaskID>) {
        self.tasks.extend(task_ids);
        self.tasks.sort();
        self.tasks.dedup();
    }
    pub fn block_by_external(&mut self, external_reason: ExternalBlockingReason) {
        self.externals.push(external_reason);
    }
    pub fn unblock_task(&mut self, task_id: TaskID) -> bool {
        self.tasks.retain(|t| t != &task_id);
        self.is_ready()
    }
    pub fn unblock_external(&mut self, reason_index: usize) -> bool {
        if reason_index < self.externals.len() {
            self.externals.remove(reason_index);
        }
        self.is_ready()
    }
    pub fn is_ready(&self) -> bool {
        self.tasks.is_empty() && self.externals.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Ready,
    Blocked(BlockingStatus),
    Completed(NaiveDateTime),
    Dropped,
}

impl Task {
    pub fn new(title: String, deadline: Option<Deadline>, note: Option<String>) -> Self {
        Self {
            id: TaskID::new(),
            title,
            created_at: chrono::Local::now().naive_local(),
            deadline: deadline.unwrap_or(Deadline::Unknown),
            status: TaskStatus::Ready,
            note,
            estimate: None,
            progress: None,
            actual_total: Duration::zero(),
        }
    }
    pub fn remaining(&self) -> Duration {
        match (&self.estimate, self.progress, self.actual_total) {
            (Some(estimate), Some(progress), actual_total) if actual_total.is_zero() => {
                // 見積と進捗があるが実績時間がない場合、残りの進捗率と見積から計算
                let progress: u8 = progress.into();
                let estimate = estimate.mean();
                estimate - (estimate / 100 * progress.into())
            }
            (_, Some(progress), actual_total) => {
                // 進捗と実績時間がある場合、見積の有無にかかわらず実績時間と今までの進捗から今後のペースを計算
                let progress: u8 = progress.into();
                (actual_total / progress.into()) * (100 - progress).into()
            }
            (Some(estimate), None, actual_total) => {
                // 見積があるが進捗がない場合、見積から実績時間を引いたものを残り時間とする
                estimate.mean() - actual_total
            }
            _ => {
                if self.is_completed() || self.is_dropped() {
                    // 完了またはドロップされたタスクは残り時間をゼロとする
                    Duration::zero()
                } else {
                    // 見積も進捗も実績時間もない場合、5分を残り時間とする
                    Duration::minutes(5)
                }
            }
        }
    }
    pub fn update_remaining(&mut self, estimated_remaining: Estimate) -> Result<(), String> {
        if !self.is_ready() && !self.is_blocked() {
            return Err("Cannot update estimate for a non-ready task".to_string());
        }
        self.estimate = Some(estimated_remaining + Estimate::new(self.actual_total));
        self.progress = None; // 見積もりを更新したら進捗オーバーライドはリセット
        Ok(())
    }
    pub fn progress(&self) -> Progress {
        match self.progress {
            Some(progress) => progress,
            None => match &self.estimate {
                Some(estimate) => Progress::new((self.actual_total.num_minutes() * 100 / estimate.mean().num_minutes()) as u8).unwrap(),
                None => Progress::zero(),
            },
        }
    }
    pub fn status(&self) -> &TaskStatus {
        &self.status
    }
    pub fn is_ready(&self) -> bool {
        matches!(self.status, TaskStatus::Ready)
    }
    pub fn is_blocked(&self) -> bool {
        matches!(self.status, TaskStatus::Blocked { .. })
    }
    pub fn is_completed(&self) -> bool {
        matches!(self.status, TaskStatus::Completed(_))
    }
    pub fn is_dropped(&self) -> bool {
        matches!(self.status, TaskStatus::Dropped)
    }
    pub fn estimate(&self) -> Option<&Estimate> {
        self.estimate.as_ref()
    }
    pub fn drop(&mut self) {
        self.status = TaskStatus::Dropped;
    }
    pub fn record(&mut self, duration: Duration) {
        self.actual_total += duration;
    }
    pub fn complete(&mut self, completed_at: NaiveDateTime) {
        self.progress = Some(Progress::full());
        self.status = TaskStatus::Completed(completed_at);
    }
    pub fn block_by_task(&mut self, task_ids: Vec<TaskID>) {
        if let TaskStatus::Blocked(status) = &mut self.status {
            status.block_by_task(task_ids);
        } else {
            self.status = TaskStatus::Blocked(BlockingStatus::by_task(task_ids));
        }
    }
    pub fn block_by_external(&mut self, external_reason: ExternalBlockingReason) {
        if let TaskStatus::Blocked(status) = &mut self.status {
            status.block_by_external(external_reason);
        } else {
            self.status = TaskStatus::Blocked(BlockingStatus::by_external(external_reason));
        }
    }
    pub fn unblock_task(&mut self, task_id: TaskID) {
        if let TaskStatus::Blocked(status) = &mut self.status {
            status.unblock_task(task_id);
            if status.is_ready() {
                self.status = TaskStatus::Ready;
            }
        }
    }
    pub fn unblock_external(&mut self, reason_index: usize) {
        if let TaskStatus::Blocked(status) = &mut self.status {
            status.unblock_external(reason_index);
            if status.is_ready() {
                self.status = TaskStatus::Ready;
            }
        }
    }
    pub fn simulate_progress(&self, duration: &Duration) -> Result<Progress, String> {
        let estimate = self.estimate.as_ref().ok_or("Estimate is not set")?.mean();
        let progress: u8 = self.progress.unwrap_or_default().into();
        let current_time = estimate / 100 * progress.into();
        let total_time = current_time + *duration;
        let new_progress = 100.0 * total_time.num_minutes() as f64 / estimate.num_minutes() as f64;

        Progress::try_from(new_progress as u8)
    }
}

#[test]
fn test_simulate_progress() {
    let mut task = Task::new("Test Task".to_string(), None, None);
    let estimate = Estimate::new(Duration::minutes(200));
    task.update_remaining(estimate);
    task.progress = Some(Progress::new(20).unwrap());
    let duration = Duration::minutes(50);
    let progress = task.simulate_progress(&duration).unwrap();
    assert_eq!(progress.0, 45);
}

#[test]
fn test_remaining() {
    let task_base = Task::new("Test Task".to_string(), None, None);
    {
        // 見積も進捗も実績時間もない場合
        let task = task_base.clone();
        assert_eq!(task.remaining(), Duration::zero());
    }
    {
        // 見積と進捗はあるが実績時間がない場合
        let mut task = task_base.clone();
        task.update_remaining(Estimate::new(Duration::minutes(200)));
        task.progress = Some(Progress::new(20).unwrap());
        assert_eq!(task.remaining(), Duration::minutes(160));
    }
    {
        // 見積はあるが進捗も実績時間もない場合
        let mut task = task_base.clone();
        task.update_remaining(Estimate::new(Duration::minutes(200)));
        assert_eq!(task.remaining(), Duration::minutes(200));
    }
    {
        // 進捗と実績時間がある場合 (見積の有無は関係ない)
        let mut task = task_base.clone();
        task.progress = Some(Progress::new(20).unwrap());
        task.actual_total = Duration::minutes(40);
        assert_eq!(task.remaining(), Duration::minutes(160));
    }
}
