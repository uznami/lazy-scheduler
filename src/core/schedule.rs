use super::{
    calendar::Calendar,
    slot::SlotMap,
    task::{Task, TaskID, TaskStatus},
};
use crate::core::{deadline::Deadline, utils::format_human_duration};
use chrono::{Duration, NaiveDateTime, NaiveTime};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet},
};

/// すべてのタスクについて「いつから着手可能か」を計算して返す。
/// - `now`：現時点
/// - `calendar`：公式稼働日情報を含む
/// - `default_time`：外部期限／Fuzzy解決時の時刻
fn compute_earliest_start_map(
    tasks: &BTreeMap<TaskID, Task>,
    calendar: &Calendar,
    now: NaiveDateTime,
    default_time: NaiveTime,
    work_tick: Duration,
    buffer: Duration,
) -> HashMap<TaskID, NaiveDateTime> {
    let mut earliest = HashMap::new();
    struct Context<'a> {
        tasks: &'a BTreeMap<TaskID, Task>,
        calendar: &'a Calendar,
        now: NaiveDateTime,
        default_time: NaiveTime,
        work_tick: Duration,
        buffer: Duration,
    }
    let context = Context {
        tasks,
        calendar,
        now,
        default_time,
        work_tick,
        buffer,
    };

    // 再帰的に個々のタスクの着手可能時刻を求める
    fn dfs(task_id: &TaskID, ctx: &Context, memo: &mut HashMap<TaskID, NaiveDateTime>) -> NaiveDateTime {
        if let Some(&t) = memo.get(task_id) {
            return t; // メモにあればそれを返す
        }
        let task = &ctx.tasks[task_id];
        let mut earliest = ctx.now;
        if let TaskStatus::Blocked(bs) = task.status() {
            // 1) 外部ブロッキング解除時刻
            // ExternalBlockingReason の may_unblock_at を解決して最大値を取る
            for ext in &bs.externals {
                let Some(unblock_time) = ext.may_unblock_at.resolve_with_calendar(ctx.calendar, ctx.default_time).expect("カレンダーで解決失敗") else {
                    continue;
                };
                earliest = earliest.max(unblock_time);
            }
            // 2) 依存タスクの完了時刻 or 再帰的着手可能時刻
            for dep_task_id in &bs.tasks {
                // dep タスクが完了していればその完了日時、それ以外は
                // 再帰的に「そのタスクが着手可能になる時刻」を使う
                let dep_task = &ctx.tasks[dep_task_id];
                let unblock_time = match dep_task.status() {
                    TaskStatus::Completed(dt) => *dt,
                    _ => {
                        // まだ終わっていない依存タスクは、着手可能時刻 + 残作業時間をカレンダー＋労働時間でシミュレート
                        let dep_start = dfs(dep_task_id, ctx, memo);
                        project_finish(dep_start, dep_task.remaining(), ctx.calendar, ctx.work_tick, ctx.buffer)
                    }
                };
                earliest = earliest.max(unblock_time);
            }
        }
        memo.insert(*task_id, earliest);
        earliest
    }

    for id in tasks.keys() {
        dfs(id, &context, &mut earliest);
        println!("earliest[{}] = {}", id, earliest[id]);
    }
    earliest
}

/// 全タスクの「最遅開始時刻」を計算する
fn compute_latest_start_map(
    tasks: &BTreeMap<TaskID, Task>,
    rev_graph: &HashMap<TaskID, Vec<TaskID>>,
    calendar: &Calendar,
    default_time: NaiveTime,
    work_tick: Duration,
    buffer: Duration,
) -> HashMap<TaskID, NaiveDateTime> {
    // 締切を起点に、後ろ向きに propagate
    let mut latest: HashMap<_, NaiveDateTime> = HashMap::new();

    // 1) 末端（explicit deadline があるもの）はまず埋める
    for (&id, task) in tasks {
        if let Some(dl_dt) = task.deadline.resolve_with_calendar(calendar, default_time).expect("カレンダーで解決失敗") {
            // 締切時刻から逆シミュレートして開始時刻を算出
            latest.insert(id, project_start_before(dl_dt, task.remaining(), calendar, work_tick, buffer));
        }
    }
    // 2) 逆トポロジカル順で伝播
    fn dfs(id: TaskID, tasks: &BTreeMap<TaskID, Task>, rev: &HashMap<TaskID, Vec<TaskID>>, latest: &mut HashMap<TaskID, NaiveDateTime>, calendar: &Calendar, work_tick: Duration, buffer: Duration) {
        if latest.contains_key(&id) {
            return;
        }
        // 子ノードを先に処理
        if let Some(children) = rev.get(&id) {
            for &ch in children {
                dfs(ch, tasks, rev, latest, calendar, work_tick, buffer)
            }
            // 子タスクの earliest 最新を取る
            let min_child = children.iter().filter_map(|&ch| latest.get(&ch)).cloned().min().unwrap();
            // 自分の残作業から逆算
            let start = project_start_before(min_child, tasks[&id].remaining(), calendar, work_tick, buffer);
            latest.insert(id, start);
        } else {
            // 締切なし＆子もない → カレンダーの最大値を入れる
            let last_window = calendar.time_windows_rev(NaiveDateTime::MAX).find(|w| w.available()).unwrap();
            let start = last_window.end - tasks[&id].remaining();
            latest.insert(id, last_window.date.and_time(start));
        }
    }
    for &id in tasks.keys() {
        dfs(id, tasks, rev_graph, &mut latest, calendar, work_tick, buffer);
    }
    latest
}

/// タスクの逆依存グラフを構築する
/// dep -> Vec<dependent>
pub fn build_rev_graph(tasks: &BTreeMap<TaskID, Task>) -> HashMap<TaskID, Vec<TaskID>> {
    let mut rev_graph: HashMap<TaskID, Vec<TaskID>> = HashMap::new();
    for (&id, task) in tasks.iter() {
        if let TaskStatus::Blocked(bs) = task.status() {
            for &dep in &bs.tasks {
                rev_graph.entry(dep).or_default().push(id);
            }
        }
    }
    rev_graph
}

/// 各タスクID ごとに「何個のタスクがこれに依存しているか」を数えて返す。
pub fn compute_dependents_map(tasks: &BTreeMap<TaskID, Task>, rev_graph: &HashMap<TaskID, Vec<TaskID>>) -> HashMap<TaskID, usize> {
    // ID ごとに「下流ノード集合」を記憶するメモ
    let mut memo: HashMap<TaskID, HashSet<TaskID>> = HashMap::new();

    fn dfs(id: TaskID, rev: &HashMap<TaskID, Vec<TaskID>>, memo: &mut HashMap<TaskID, HashSet<TaskID>>) -> HashSet<TaskID> {
        if let Some(cached) = memo.get(&id) {
            return cached.clone();
        }
        memo.insert(id, HashSet::new()); // サイクル防御
        let mut all = HashSet::new();
        if let Some(children) = rev.get(&id) {
            for &ch in children {
                all.insert(ch);
                all.extend(dfs(ch, rev, memo));
            }
        }
        memo.insert(id, all.clone());
        all
    }

    tasks
        .keys()
        .map(|&id| {
            let deps = dfs(id, rev_graph, &mut memo);
            (id, deps.len())
        })
        .collect()
}

/// start: 着手可能時刻
/// rem:  残作業時間 (Duration)
/// calendar: 公式稼働日情報
/// buffer: タスク間バッファ (Duration)
fn project_finish(start: NaiveDateTime, mut remaining: Duration, calendar: &Calendar, work_tick: Duration, buffer: Duration) -> NaiveDateTime {
    for window in calendar.time_windows(start).filter(|w| w.available()) {
        // このウィンドウの実際の開始点は max(start, window.start)
        let mut cursor = window.start_datetime().max(start);
        let end = window.end_datetime();

        while cursor < end && remaining > Duration::zero() {
            // このウィンドウで使える時間
            let slot = (end - cursor).min(work_tick); // バッファは次ループで cursor に追加
            // 作業分
            let work = slot.min(remaining);
            cursor += work;
            remaining -= work;
            // 終了後のバッファ
            cursor += buffer;
        }

        if remaining <= Duration::zero() {
            return cursor - buffer; // 完了時間 (最後のバッファを引く)
        }
    }
    // もし free_time で全部消化できなかったら、最後に start+rem としておく
    start + remaining
}

/// project_finish の逆版：終点から buffer/work_tick を遡って開始時刻を返す
fn project_start_before(finish: NaiveDateTime, mut remaining: Duration, calendar: &Calendar, work_tick: Duration, buffer: Duration) -> NaiveDateTime {
    // cursor は finish からスタート
    let mut cursor = finish;
    for window in calendar.time_windows_rev(finish).filter(|w| w.available()) {
        let win_start = window.start_datetime();
        let win_end = window.end_datetime().min(cursor);
        let mut t = win_end;

        // このウィンドウ内で remaining を消化
        while t > win_start && remaining > Duration::zero() {
            // 直前のバッファを含めたスロット長
            let slot = (t - win_start).min(work_tick);
            let work = slot.min(remaining);
            t -= work + buffer; // 作業とバッファ分遡る
            remaining -= work;
        }

        // もし全部消化できたら t+buffer が開始時刻
        if remaining <= Duration::zero() {
            return t + buffer;
        }
        // 次のウィンドウはこのウィンドウの開始時刻より前
        cursor = win_start;
    }

    // free_time で足りなければ単純に finish-remaining
    finish - remaining
}

#[test]
fn test_compute_dependents_map() {
    // サンプルタスクをBTreeMapで作成
    // A → B, A → C, B → D の依存
    // 従って、A の dependents_count = 3, B = 1, C = 0, D = 0
    let mut tasks = BTreeMap::new();
    let id_a = TaskID::new();
    let id_b = TaskID::new();
    let id_c = TaskID::new();
    let id_d = TaskID::new();

    let mut ta = Task::new("A".into(), None, None);

    let mut tb = Task::new("B".into(), None, None);
    tb.block_by_task(vec![id_a]);

    let mut tc = Task::new("C".into(), None, None);
    tc.block_by_task(vec![id_a]);

    let mut td = Task::new("D".into(), None, None);
    td.block_by_task(vec![id_b]);

    tasks.insert(id_a, ta);
    tasks.insert(id_b, tb);
    tasks.insert(id_c, tc);
    tasks.insert(id_d, td);

    let rev_graph = build_rev_graph(&tasks);
    let dep_map = compute_dependents_map(&tasks, &rev_graph);
    assert_eq!(dep_map[&id_a], 3);
    assert_eq!(dep_map[&id_b], 1);
    assert_eq!(dep_map[&id_c], 0);
    assert_eq!(dep_map[&id_d], 0);
}

struct ScheduleContext<'a> {
    /// 1日の総勤務時間（分）
    daily_minutes: f64,
    /// スケジュール開始時刻
    now: NaiveDateTime,
    /// タスクマップ
    tasks: &'a BTreeMap<TaskID, Task>,
    /// カレンダー
    calendar: &'a Calendar,
    /// 各タスクの着手可能時刻
    earliest: HashMap<TaskID, NaiveDateTime>,
    /// 各タスクの着手可能時刻（最遅）
    latest: HashMap<TaskID, NaiveDateTime>,
    /// 各タスクの必要日数
    need: HashMap<TaskID, f64>,
    /// 逆依存グラフ
    rev_graph: HashMap<TaskID, Vec<TaskID>>,
    /// 各タスクの依存度
    dep_map: HashMap<TaskID, usize>,
    /// 最大依存度
    max_dep: f64,
    /// 各タスクのリスク（平均・標準偏差）
    risk_map: HashMap<TaskID, (f64, f64)>,
    working_time: (NaiveTime, NaiveTime),

    /// スロットマップ
    slots: SlotMap,
    /// 各タスクの残り時間（分）
    remaining_minutes: HashMap<TaskID, i64>,
}

impl<'a> ScheduleContext<'a> {
    /// 各タスクの「残り作業時間」を、(1日の勤務時間) で割って
    /// 必要な日数（端数は切り上げ）を f64 で返す。
    fn compute_need_days_map(tasks: &BTreeMap<TaskID, Task>, daily_minutes: f64) -> HashMap<TaskID, f64> {
        let mut map = HashMap::new();

        for (&id, task) in tasks.iter() {
            // まず残り時間（分）を取得
            let rem_min = task.remaining().num_minutes() as f64;
            // 0分以下なら 0 日
            let need_days = if rem_min <= 0.0 {
                0.0
            } else {
                // 分単位 → "日数" に変換
                (rem_min / daily_minutes)
            };
            map.insert(id, need_days);
        }

        map
    }

    fn build(now: NaiveDateTime, tasks: &'a BTreeMap<TaskID, Task>, calendar: &'a Calendar, working_time: &(NaiveTime, NaiveTime), work_tick: Duration, buffer_time: Duration) -> Self {
        // 前準備：着手可能時刻・必要日数・依存度・リスクを一度計算
        let daily_minutes = (working_time.1 - working_time.0).num_minutes() as f64;
        let now = calendar.official_workdays(now.date()).next().cloned().unwrap_or(now.date()).and_time(working_time.0);
        let need = Self::compute_need_days_map(tasks, daily_minutes);
        let rev_graph = build_rev_graph(tasks);
        let earliest = compute_earliest_start_map(tasks, calendar, now, working_time.0, work_tick, buffer_time);
        let latest = compute_latest_start_map(tasks, &rev_graph, calendar, working_time.0, work_tick, buffer_time);
        let dep_map = compute_dependents_map(tasks, &rev_graph);
        let max_dep = dep_map.values().cloned().fold(0, usize::max).max(1) as f64;
        let risk_map: HashMap<_, (f64, f64)> = tasks
            .iter()
            .map(|(&id, t)| {
                let (m, s) = t.estimate().map(|e| (e.mean().num_minutes() as f64, e.stddev().num_minutes() as f64)).unwrap_or((0.0, 0.0));
                (id, (m, s))
            })
            .collect();
        let remaining_minutes = need.iter().map(|(&id, &days)| ((id), (days * daily_minutes).ceil() as i64)).collect::<HashMap<_, _>>();
        let mut slots = SlotMap::new();

        Self {
            now,
            tasks,
            calendar,
            earliest,
            latest,
            need,
            rev_graph,
            dep_map,
            max_dep,
            risk_map,
            working_time: *working_time,
            daily_minutes,
            slots: SlotMap::new(),
            remaining_minutes,
        }
    }

    /// スラック (余裕時間) を計算する
    /// - `id`：タスクID
    /// - `cursor`：現在時刻
    fn calc_slack(&self, id: &TaskID, cursor: &NaiveDateTime) -> f64 {
        let slack = (self.latest[id] - *cursor).num_minutes() as f64 / self.daily_minutes;
        if slack.is_finite() { slack } else { 0.0 }
    }

    /// 全タスクの最大スラックを計算する
    fn calc_max_slack_on(&self, cursor: &NaiveDateTime) -> f64 {
        // その中で最大のものを返す
        self.tasks
            .keys()
            .filter(|&&id| self.remaining_minutes[&id] > 0 && self.earliest[&id] <= *cursor)
            .map(|&id| self.calc_slack(&id, cursor))
            .fold(1.0_f64, f64::max)
    }

    /// タスクの優先度を計算する
    fn calc_priority_score(&self, id: &TaskID, cursor: &NaiveDateTime, max_slack: f64) -> (f64, f64) {
        // 1) 依存度
        let d_score = self.dep_map.get(id).cloned().unwrap_or(0) as f64 / self.max_dep;
        // 2) リスク
        let (m, s) = self.risk_map[id];
        let r_score = if m > 0.0 { s / m } else { 0.0 };
        // 3) 緊急度
        let slack = (self.latest[id] - *cursor).num_minutes() as f64 / self.daily_minutes;
        let urgency = if slack.is_finite() { (1.0 - (slack / max_slack)).clamp(0.001, 1.0) } else { 0.0 };
        (urgency, 0.7 * r_score + 0.3 * d_score)
    }

    /// タスクをスロットに割り当てる
    fn allocate(&mut self, task_id: &TaskID, work_tick: &Duration, cursor: &NaiveDateTime, capacity: &Duration) -> Duration {
        let alloc = Duration::minutes(self.remaining_minutes[task_id]).min(*work_tick).min(*capacity);
        self.slots.add(cursor.date(), *task_id, alloc);
        self.remaining_minutes.entry(*task_id).and_modify(|m| *m = (*m - alloc.num_minutes()).max(0));
        alloc
    }

    /// 全タスクの中で最も早く着手できるタスクの着手可能時刻を取得する
    fn find_first_allocatable_time(&self, from: &NaiveDateTime, to: &NaiveDateTime) -> Option<NaiveDateTime> {
        self.tasks
            .keys()
            .filter(|&&id| self.remaining_minutes[&id] > 0)
            .map(|&id| self.earliest[&id])
            .filter(|&t| t > *from && t < *to)
            .min()
    }
}

#[derive(Debug)]
pub struct Scheduler {
    pub work_tick: Duration,
    pub buffer_time: Duration,
    pub working_time: (NaiveTime, NaiveTime),
}

impl Scheduler {
    /// 依存・外部ブロック・締切・不確実性を考慮して
    /// 空きウィンドウにタスクを貪欲割当します。
    ///
    /// - `now`：現在日時
    /// - `tasks`：全タスクマップ
    /// - `calendar`：公式稼働日カレンダー
    pub fn schedule(&self, now: NaiveDateTime, tasks: &BTreeMap<TaskID, Task>, calendar: &Calendar) -> anyhow::Result<SlotMap> {
        let mut context = ScheduleContext::build(now, tasks, calendar, &self.working_time, self.work_tick, self.buffer_time);

        // free windows ループ
        for window in calendar.time_windows(now) {
            if !window.available() {
                println!("{} {}-{}: {}", window.date, window.start.format("%H:%M"), window.end.format("%H:%M"), window.note());
                continue;
            }
            let mut cursor = window.start_datetime();
            let mut capacity = window.end - window.start;

            // 量子ごとに動的プライオリティ再計算
            while capacity > Duration::zero() {
                // (A) 現時刻で着手可能かつ未完了なタスクだけ取り出す
                let mut best = None;
                // 最大スラックの取得（動的再計算用）
                let max_slack = context.calc_max_slack_on(&cursor);

                for &id in tasks.keys() {
                    let already_done = context.remaining_minutes[&id] <= 0;
                    let cannot_start_yet = context.earliest[&id] > cursor;
                    if already_done || cannot_start_yet {
                        continue;
                    }
                    let score = context.calc_priority_score(&id, &cursor, max_slack);
                    if best.as_ref().is_none_or(|&(bs, _)| score > bs) {
                        best = Some((score, id));
                    }
                }

                // 割り当て
                if let Some((_, chosen)) = best {
                    // 割り当て可能なタスクがあれば、スロットに追加して、残り時間を減らし、時間を進める
                    let alloc = context.allocate(&chosen, &self.work_tick, &cursor, &capacity);
                    println!(
                        "{} {}-{}: {} ({}分)",
                        cursor.date(),
                        cursor.time().format("%H:%M"),
                        (cursor + alloc).time().format("%H:%M"),
                        context.tasks[&chosen].title,
                        alloc.num_minutes()
                    );
                    let consumed = alloc + self.buffer_time;
                    capacity -= consumed;
                    cursor += consumed;
                } else {
                    // 現時点で割り当て可能なタスクがない場合: 最速で着手可能なタスクの開始時刻がウィンドウ内にあれば、その時刻に移動
                    if let Some(earliest_allocatable_time) = context.find_first_allocatable_time(&cursor, &window.end_datetime()) {
                        capacity = window.end_datetime() - cursor;
                        cursor = earliest_allocatable_time;
                        continue;
                    }
                    // ウィンドウ内に新しい候補がなければ終了
                    break;
                }
            }
        }

        Ok(context.slots)
    }
}
