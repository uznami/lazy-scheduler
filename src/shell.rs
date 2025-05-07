use core::panic;
use std::default;

use crate::core::{
    deadline::{self, Deadline, FuzzyDeadline, FuzzyDeadlineKind},
    estimate::Estimate,
    session,
    task::{ExternalBlockingReason, Progress, Task, TaskStatus},
    utils::{StopKind, format_human_duration, parse_human_duration, parse_human_duration_with_sign, parse_stop_kind},
};
use anyhow::{anyhow, bail};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, format, naive};
use regex::Regex;

const TASKS_FILE: &str = "tasks.json";

fn task_status_symbol(task: &Task) -> &'static str {
    if task.is_ready() {
        "⬜"
    } else if task.is_blocked() {
        "⌛"
    } else if task.is_completed() {
        "✅"
    } else if task.is_dropped() {
        "❌"
    } else {
        panic!("Unknown task status");
    }
}

pub fn parse_deadline<'a>(now: NaiveDateTime, default_deadline_time: NaiveTime, mut parts: impl Iterator<Item = &'a str>) -> anyhow::Result<Deadline> {
    let Some(first) = parts.next() else {
        bail!("deadline を指定してください");
    };

    match first {
        "on" => {
            // 次のトークンを取って解釈
            let tok = parts.next().ok_or_else(|| anyhow!("on の後に日時を指定してください (例: on 2025-05-10 14:00 または on 14:30)"))?;
            // 時刻だけ ("HH:MM" or "HH:MM:SS")
            let maybe_time = NaiveTime::parse_from_str(tok, "%H:%M:%S").or_else(|_| NaiveTime::parse_from_str(tok, "%H:%M")).ok();
            let (date, time) = if let Some(t) = maybe_time {
                // time-only → 今日の日付 + 指定時刻
                (now.date(), t)
            } else {
                // 日付ありパターン
                // 1) YYYY-MM-DD
                // 2) YYYY/MM/DD
                // 3) MM/DD (年省略 → now.year())
                let date = if tok.contains('-') {
                    NaiveDate::parse_from_str(tok, "%Y-%m-%d").map_err(|_| anyhow!("日付形式は YYYY-MM-DD で指定してください"))?
                } else if tok.contains('/') {
                    let parts: Vec<_> = tok.split('/').collect();
                    match parts.as_slice() {
                        [y, m, d] => {
                            // YYYY/MM/DD
                            NaiveDate::parse_from_str(tok, "%Y/%m/%d").map_err(|_| anyhow!("日付形式は YYYY/MM/DD で指定してください"))?
                        }
                        [m, d] => {
                            // MM/DD → 今の年
                            let year = now.year();
                            NaiveDate::from_ymd_opt(year, m.parse().map_err(|_| anyhow!("月が不正です"))?, d.parse().map_err(|_| anyhow!("日が不正です"))?).ok_or_else(|| anyhow!("無効な日付です"))?
                        }
                        _ => bail!("日付形式は YYYY-MM-DD, YYYY/MM/DD, MM/DD のいずれかです"),
                    }
                } else {
                    bail!("日付形式が不正です: {}", tok);
                };

                // オプションで続くトークンを時刻として解釈
                let next_tok = parts.next();
                let time = if let Some(ts) = next_tok {
                    NaiveTime::parse_from_str(ts, "%H:%M:%S")
                        .or_else(|_| NaiveTime::parse_from_str(ts, "%H:%M"))
                        .map_err(|_| anyhow!("時刻形式は HH:MM(:SS) で指定してください"))?
                } else {
                    // 時刻未指定 → デフォルト
                    default_deadline_time
                };
                (date, time)
            };

            Ok(Deadline::Exact(date.and_time(time)))
        }
        "none" => Ok(Deadline::None),
        "unknown" => Ok(Deadline::Unknown),
        "in" => {
            let duration_str = parts.next().ok_or_else(|| anyhow!("duration が必要です (例: 3d, 5h)"))?.trim().to_lowercase();
            let (num_str, unit) = duration_str.split_at(duration_str.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(duration_str.len()));
            let value: f64 = num_str.parse().map_err(|_| anyhow!("数値部分が不正です"))?;
            let mins = match unit.trim() {
                "m" | "min" | "mins" => value,
                "h" | "hr" | "hrs" => value * 60.0,
                "d" | "day" | "days" => value * 60.0 * 24.0,
                "w" | "week" | "weeks" => value * 60.0 * 24.0 * 7.0,
                "mo" | "month" | "months" => value * 60.0 * 24.0 * 30.0,
                _ => bail!("不明な単位: {}", unit),
            };
            let duration = Duration::minutes(mins.round() as i64);
            let mut deadline = now + duration;
            println!("raw deadline: {}", deadline);
            if Duration::hours(12) < duration {
                deadline = deadline.date().and_time(default_deadline_time); // 12時間以上のdurationは、日付指定のみ採用して時間はデフォルト
            }
            Ok(Deadline::Exact(deadline))
        }
        "about" => {
            let raw = parts.next().ok_or_else(|| anyhow!("about の形式は about <n><unit> です"))?;
            if parts.next().is_some() {
                bail!("about の形式は about <n><unit> です（空白を入れずに書いてください）");
            }
            let (digits, unit) = raw.chars().partition::<String, _>(|c| c.is_ascii_digit());
            if digits.is_empty() || unit.is_empty() {
                bail!("数値と単位が正しく含まれていません");
            }
            let n: u16 = digits.parse().map_err(|_| anyhow!("数値部分が不正です"))?;
            let kind = match unit.as_str() {
                "bd" | "bday" | "bdays" => FuzzyDeadlineKind::BusinessDays(n),
                "fri" | "friday" => FuzzyDeadlineKind::FridayOfWeeks(n),
                "w" | "weeks" => FuzzyDeadlineKind::Weeks(n),
                "me" | "monthend" | "monthends" => FuzzyDeadlineKind::MonthEnds(n),
                "m" | "month" | "months" => FuzzyDeadlineKind::Months(n),
                _ => bail!("不明な単位: {}", unit),
            };
            Ok(Deadline::Fuzzy(FuzzyDeadline::new(now, kind, None)))
        }
        _ => bail!("期限の指定形式が不明です: {}", first),
    }
}

pub fn handle_block_by_task(session: &mut session::Session, args: Vec<&str>) -> anyhow::Result<()> {
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("ID is required for block command");
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let dependencies = args
        .iter()
        .skip(1)
        .map(|arg| {
            let id_key = arg.trim();
            if id_key.is_empty() {
                bail!("ID is required for block command");
            }
            let Some(tid) = session.find_task_by_prefix(id_key) else {
                bail!("⚠️タスク{}が見つかりません。", id_key);
            };
            if task_id == tid {
                return Ok(None);
            }
            Ok(Some(tid))
        })
        .filter_map(|x| x.transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let (task, dependencies) = session.block_task_by_tasks(&task_id, dependencies);
    println!("⌛ ブロッキング: {} - {}", task.id, task.title);
    if dependencies.is_empty() {
        println!("  依存タスクなし");
    } else {
        println!("  依存タスク:");
        for dep in dependencies {
            println!("    - {}", dep.title);
        }
    }
    Ok(())
}

fn handle_block_by_external(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("ID is required for block command");
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let deadline = parse_deadline(now, session.scheduler.working_time.0, args.iter().skip(1).copied())?;
    let task = session.block_task_by_external(&task_id, now, deadline, None);
    println!("⌛ ブロッキング: {} - {}", task.id, task.title);
    Ok(())
}

fn handle_add(session: &mut session::Session, args: Vec<&str>) -> anyhow::Result<()> {
    let title: String = args.join(" ");
    if title.is_empty() {
        bail!("Title is required for add command");
    }
    let task = Task::new(title.clone(), None, None);
    let task = session.add_task(task);
    println!("✅ 追加: {} - {}", task.id, task.title);
    Ok(())
}

fn handle_list(session: &mut session::Session, _now: NaiveDateTime, _args: Vec<&str>) -> anyhow::Result<()> {
    if session.iter_tasks().next().is_none() {
        println!("(タスクなし)");
    } else {
        let println_task = |task: &Task| {
            println!("    {} {}", task.id, task.title);
            let remaining = task.remaining();
            if let Some(estimate) = task.estimate() {
                if estimate.stddev().num_minutes() > 0 {
                    println!(
                        "      予想: {} (最尤{}, 楽観{}, 最悪{}, σ={})",
                        format_human_duration(estimate.mean()),
                        format_human_duration(estimate.most_likely),
                        format_human_duration(estimate.optimistic),
                        format_human_duration(estimate.pessimistic),
                        format_human_duration(estimate.stddev())
                    );
                } else {
                    println!("      予想: {}", format_human_duration(estimate.mean()));
                }
            }
            if !task.actual_total.is_zero() {
                println!(
                    "      実績: {} (進捗{}, 予想残り時間: {})",
                    format_human_duration(task.actual_total),
                    task.progress(),
                    format_human_duration(task.remaining())
                );
            }
            let deadline = match &task.deadline {
                Deadline::None => {
                    println!("      期限: なし");
                    None
                }
                Deadline::Unknown => {
                    println!("      期限: 不明");
                    None
                }
                Deadline::Exact(naive_date_time) => {
                    print!("      期限: {}(絶対)", naive_date_time);
                    Some(*naive_date_time)
                }
                Deadline::Fuzzy(fuzzy_deadline) => {
                    let default_deadline_time = session.scheduler.working_time.0;
                    let dl = fuzzy_deadline.resolve_with_calendar(&session.calendar, default_deadline_time).unwrap();
                    print!("      期限: {}(相対)", dl);
                    Some(dl)
                }
            };
            if let Some(deadline) = deadline {
                let remaining = deadline.signed_duration_since(chrono::Local::now().naive_local());
                if remaining.num_minutes() < 0 {
                    println!("({}超過⚠️)", format_human_duration(-remaining));
                } else {
                    println!("(あと{})", format_human_duration(remaining));
                }
            }
            if let TaskStatus::Blocked(bs) = task.status() {
                if !bs.externals.is_empty() {
                    println!("      外部待ち:");
                    for reason in bs.externals.iter() {
                        let may_unblock_at = reason.may_unblock_at.resolve_with_calendar(&session.calendar, session.scheduler.working_time.0).unwrap();
                        println!("        {:?}: {}", reason.note, may_unblock_at.map(|d| d.to_string() + "まで").unwrap_or_else(|| "不明".to_string()));
                    }
                }
                if !bs.tasks.is_empty() {
                    println!("      別タスク待ち:");
                    for task_id in bs.tasks.iter() {
                        println!("        {}: {}", task_id, session.tasks.get(task_id).unwrap().title);
                    }
                }
            }
            println!();
        };

        // Ready
        println!("📝 進行中のタスク:");
        for task in session.iter_tasks().filter(|t| t.is_ready()) {
            println_task(task);
        }
        // Blocked
        println!("\n⌛ ブロッキング中のタスク:");
        let blocked_tasks = session.iter_tasks().filter(|t| t.is_blocked()).collect::<Vec<_>>();
        if blocked_tasks.is_empty() {
            println!("  (ブロッキング中のタスクはありません)");
        } else {
            for task in blocked_tasks.iter() {
                println_task(task);
            }
        }
        // Completed
        println!("\n✅ 完了したタスク:");
        for task in session.iter_tasks().filter(|t| t.is_completed()) {
            println_task(task);
        }
    }
    Ok(())
}
fn handle_start(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("<task-id> を指定してください");
    }
    if let Some((tid, _)) = session.active_task {
        println!("ℹ️ 既にタスク{}が開始されています。いずれかのコマンドで中断/完了してください: ", tid);
        println!("  stop : 現在時刻で中断 (日付またいで5h以上になる場合はエラー)");
        println!("  done  : 現在時刻で完了");
        println!("  stop in <duration> : 作業時間のみ記録して中断");
        println!("  stop at <time> : 中断時刻を記録して中断");
        println!("  stop immediately : なにも記録せず即中断");
        println!("  done in <duration> : 作業時間のみ記録して完了");
        println!("  done at <time> : 完了時刻を記録して完了");
        println!("  done immediately : なにも記録せず即完了");
        return Ok(());
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let (task, allocated) = session.start_task_at(&task_id, now);
    println!("🔥タスク{}を開始しました。", task.id);
    println!("  割り当て時間: {}", format_human_duration(allocated));
    println!("  予想完了時間: {}", now + allocated);
    Ok(())
}
fn handle_done(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let Some(stop_kind) = parse_stop_kind(&args, now) else {
        bail!("Usage: done <task-id> (at HH:MM | in <duration> | immediately)");
    };
    let task = session.stop_current_task(stop_kind, true)?;
    println!("✅ 完了: {} - {}", task.id, task.title);
    Ok(())
}
fn handle_stop(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let Some(stop_kind) = parse_stop_kind(&args, now) else {
        bail!("Usage: stop (at HH:MM | in <duration> | immediately)");
    };
    let task = session.stop_current_task(stop_kind, false)?;
    println!("⏸️ 中断: {} - {}", task.id, task.title);
    Ok(())
}
fn handle_complete(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let mut args = args.iter();
    let Some(id_key) = args.next() else {
        bail!("<task-id> を指定してください");
    };
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let duration = args.next().and_then(|arg| parse_human_duration(arg));
    let task = session.complete_task(&task_id, now, duration);
    println!("✅ 完了: {} - {}", task.id, task.title);
    Ok(())
}
fn handle_drop(session: &mut session::Session, args: Vec<&str>) -> anyhow::Result<()> {
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("ID is required for drop command");
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let task_title = session.drop_task(&task_id);
    println!("❌ 削除: {} - {}", task_id, task_title);
    Ok(())
}
fn handle_deadline(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("<task-id> を指定してください");
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let default_deadline_time = chrono::NaiveTime::from_hms_opt(17, 0, 0).unwrap();
    let deadline = parse_deadline(now, default_deadline_time, args.into_iter().skip(1))?;
    let task = session.set_deadline(&task_id, deadline);
    println!("⌛ 期限: {} - {}", task.id, task.title);
    println!("  期限: {:#?}", task.deadline);
    Ok(())
}

fn handle_estimate(session: &mut session::Session, args: Vec<&str>) -> anyhow::Result<()> {
    let task_id = if let Some((tid, _)) = session.active_task {
        tid
    } else {
        let id_key = args.first().unwrap_or(&"");
        if id_key.is_empty() {
            bail!("<task-id> を指定してください");
        }
        let Some(task_id) = session.find_task_by_prefix(id_key) else {
            bail!("⚠️タスク{}が見つかりません。", id_key);
        };
        task_id
    };
    let current_remaining = Estimate::new(session.tasks.get(&task_id).unwrap().remaining());
    let times: Vec<_> = args.iter().filter_map(|arg| parse_human_duration_with_sign(arg)).collect();
    let estimate = match (times.as_slice(), current_remaining) {
        ([(None, m)], _) => Estimate::new(*m),
        ([(None, m), (None, o), (None, p)], _) => Estimate::from_mop(*m, *o, *p).map_err(|_| anyhow!("m o p で指定してください"))?,
        ([(Some(sm), m)], curr) => curr + Estimate::new(*m * *sm),
        ([(Some(sm), m), (Some(so), o), (Some(sp), p)], curr) => curr + Estimate::from_mop(*m * *sm, *o * *so, *p * *sp).map_err(|_| anyhow!("m o p で指定してください"))?,
        _ => bail!("<most-likely> (<optimistic> <pessimistic>) の形式で指定してください"),
    };
    let task = session.estimate_task(&task_id, estimate.clone())?;
    println!("⌛ 予測: {} - {}", task.id, task.title);
    println!("  予測残り時間: {}", format_human_duration(estimate.mean()));
    Ok(())
}
fn handle_record(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let mut args = args.iter();
    let Some(id_key) = args.next() else {
        bail!("<task-id> を指定してください");
    };
    let Some(duration) = args.next().and_then(|arg| parse_human_duration(arg)) else {
        bail!("Usage: record <task-id> <duration>");
    };
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let task = session.record_task(&task_id, duration);
    println!("📝 記録: {} - {}", task.id, task.title);
    Ok(())
}
fn handle_todo(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    let today = now.date();
    let mut tasks = session.iter_tasks();

    let today_slots = session.slots.get(&today);
    if today_slots.is_empty() {
        println!("✅ 今日のタスクはありません。");
        return Ok(());
    };

    let mut todo_all = today_slots
        .iter()
        .map(|(task_id, allocated)| {
            let Some(task) = tasks.find(|t| t.id == *task_id) else {
                panic!("Task not found");
            };
            (task.clone(), allocated)
        })
        .collect::<Vec<_>>();
    if todo_all.is_empty() {
        println!("✅ 今日のタスクはありません。");
        return Ok(());
    }

    // ソート：仮で allocated 大きい順（将来は progressなど）
    todo_all.sort_by_key(|&(_, d)| std::cmp::Reverse(d));

    let todo = todo_all.iter().filter(|(t, _)| t.is_ready()).collect::<Vec<_>>();

    println!("🦥 今日やること（全{}件, ブロッキング{}件）:\n", todo_all.len(), todo_all.len() - todo.len());

    for (i, (task, allocated)) in todo.iter().enumerate() {
        let title = task.title.clone();

        let simulated_progress = match task.simulate_progress(allocated) {
            Ok(progress) => format!(" -> 本日で{}", progress),
            Err(_) => "".to_owned(),
        };

        println!(
            "#{:<2} 📝 {} [{}] (進捗: {}{})",
            i + 1,
            task.title,
            format_human_duration(**allocated),
            task.progress(),
            simulated_progress,
        );
    }

    Ok(())
}

fn handle_schedule(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    session.schedule(now)?;
    println!("✅ スケジュールを更新しました。");
    Ok(())
}

fn todo_block_by_task(session: &mut session::Session, args: Vec<&str>) -> anyhow::Result<()> {
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("ID is required for block command");
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let dependencies = args
        .iter()
        .skip(1)
        .map(|arg| {
            let id_key = arg.trim();
            if id_key.is_empty() {
                bail!("ID is required for block command");
            }
            let Some(tid) = session.find_task_by_prefix(id_key) else {
                bail!("⚠️タスク{}が見つかりません。", id_key);
            };
            if task_id == tid {
                return Ok(None);
            }
            Ok(Some(tid))
        })
        .filter_map(|x| x.transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let (task, dependencies) = session.block_task_by_tasks(&task_id, dependencies);
    println!("⌛ ブロッキング: {} - {}", task.id, task.title);
    if dependencies.is_empty() {
        println!("  依存タスクなし");
    } else {
        println!("  依存タスク:");
        for dep in dependencies {
            println!("    - {}", dep.title);
        }
    }
    Ok(())
}

fn handle_progress(session: &mut session::Session, now: NaiveDateTime, args: Vec<&str>) -> anyhow::Result<()> {
    // 指定したタスクの進捗を更新
    let id_key = args.first().unwrap_or(&"");
    if id_key.is_empty() {
        bail!("ID is required for progress command");
    }
    let Some(task_id) = session.find_task_by_prefix(id_key) else {
        bail!("⚠️タスク{}が見つかりません。", id_key);
    };
    let current_progress: u8 = session.tasks.get(&task_id).unwrap().progress().into();
    let Some(progress_str) = args.get(1).map(|s| s.trim()) else {
        bail!("Usage: progress <task-id> <progress>");
    };
    let progress = match progress_str {
        "none" => None,
        arg if arg.starts_with('+') || arg.starts_with('-') => {
            let sign: i32 = match arg.chars().next().unwrap() {
                '+' => 1,
                '-' => -1,
                _ => unreachable!(),
            };
            let diff = (arg[1..].trim().parse::<u8>()? as i32 * sign).clamp(-100, 100);
            let new_progress = (current_progress as i32 + diff).clamp(0, 100);
            let new_progress = Progress::try_from(new_progress as u8).expect("Invalid progress");
            Some(new_progress)
        }
        _ => {
            let new_progress = progress_str.parse::<u8>()?;
            let Ok(new_progress) = Progress::try_from(new_progress) else {
                bail!("Invalid progress value: {}", new_progress);
            };
            Some(new_progress)
        }
    };
    let task = session.update_progress_task(&task_id, progress);
    println!("✅ 進捗: {} - {} ({})", task.id, task.title, task.progress());
    Ok(())
}

pub fn handle_command(session: &mut session::Session, mut input: &str) -> anyhow::Result<()> {
    let mut parts = input.split_whitespace();
    let now: NaiveDateTime = if input.starts_with('@') {
        let now_str = parts.next().unwrap_or("");
        NaiveDateTime::parse_from_str(now_str, "@%Y-%m-%dT%H:%M:%S")?
    } else {
        chrono::Local::now().naive_local()
    };
    let cmd = parts.next().unwrap_or("");
    let args = parts.collect::<Vec<_>>();
    let today = now.date();

    match cmd {
        "a" | "add" => handle_add(session, args)?,
        "l" | "ls" | "list" => handle_list(session, now, args)?,
        "sta" | "start" => handle_start(session, now, args)?,
        "sto" | "stop" => handle_stop(session, now, args)?,
        "dn" | "done" => handle_done(session, now, args)?,
        "r" | "rc" | "record" => handle_record(session, now, args)?,
        "co" | "comp" | "complete" => handle_complete(session, now, args)?,
        "dr" | "drop" => handle_drop(session, args)?,
        "dl" | "deadline" => handle_deadline(session, now, args)?,
        "blt" | "block-by-task" => handle_block_by_task(session, args)?,
        "ble" | "block-by-external" => handle_block_by_external(session, now, args)?,
        "e" | "est" | "estimate" => handle_estimate(session, args)?,
        "pr" | "progress" => handle_progress(session, now, args)?,
        "sc" | "schedule" => handle_schedule(session, now, args)?,
        "t" | "todo" => handle_todo(session, now, args)?,
        "" | "help" => {
            let commands = if session.active_task.is_some() {
                vec!["add", "list", "stop", "done", "comp", "drop", "est", "help", "exit"]
            } else {
                vec!["add", "list", "start", "comp", "drop", "est", "schedule", "help"]
            };
            println!("Available commands: {}", commands.join(", "));
            println!("Usage:");
            println!("  add <title> - タスクを追加");
            println!("  list - タスクを表示");
            println!("  start <tid> - タスクを開始");
            println!("  stop - 開始したタスクを中断");
            println!("  done - 開始したタスクを完了");
            println!("  comp <tid> - タスクを完了");
            println!("  drop <tid> - タスクを削除");
            println!("  est <tid> <time> - タスクの残り時間見積もりを設定");
            println!("  dl <tid> <deadline> - タスクの期限を設定");
            println!("  r <tid> <time> - タスクの実績時間を記録");
            println!("  progress <tid> <progress> - タスクの進捗を手動で上書き");
            println!("  schedule - タスクをスケジュール");
            println!("  help - このヘルプを表示");
            println!("  exit/Ctrl+D - 終了");
            println!("  todo - 今日のTODOを表示");
        }
        unknown => bail!("Unknown command: {}", unknown),
    };
    session.schedule(now)?;
    Ok(())
}
