#![allow(unused)]
use core::{calendar::Calendar, session::Session, slot, store};
use std::io::{self, Write};

use rustyline::{config::Configurer, error::ReadlineError};
mod core;
mod shell;

const SETTINGS_DIR: &str = "./settings";
const TASKS_FILE: &str = "tasks.json";
const WORKLOG_FILE: &str = "worklog.json";
const COMMAND_HISTORY_FILE: &str = ".history";

fn main() -> anyhow::Result<()> {
    println!("🧠 LazyScheduler Shell - type 'help' to get started");

    let mut rl = rustyline::DefaultEditor::new()?;
    if std::path::Path::new(COMMAND_HISTORY_FILE).exists() {
        rl.load_history(COMMAND_HISTORY_FILE)?;
    }
    rl.set_auto_add_history(true);
    rl.set_max_history_size(1000);

    let calendar = Calendar::import_from_yaml(SETTINGS_DIR)?;
    let tasks = store::load_tasks(TASKS_FILE)?;
    let log = store::load_worklog(WORKLOG_FILE)?;
    let mut session = Session::new(calendar, tasks, log);

    loop {
        let prompt = match &session.active_task {
            Some((task_id, started_at)) => format!("{} (started at {}) > ", task_id, started_at),
            None => "> ".to_owned(),
        };
        let line = rl.readline(&prompt);
        match line {
            Err(ReadlineError::Eof) => {
                println!("👋 Bye!");
                break;
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(err) => {
                eprintln!("❌ Error reading input: {}", err);
                continue;
            }
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match trimmed {
                    "exit" | "quit" => {
                        println!("👋 Bye!");
                        break;
                    }
                    _ => {
                        if let Err(err) = shell::handle_command(&mut session, trimmed) {
                            eprintln!("❌ Error: {}", err);
                        }
                    }
                }
            }
        }
    }

    // Save tasks to file before exiting
    if session.dirty_tasks {
        if let Err(err) = store::save_tasks(&session.tasks, TASKS_FILE) {
            eprintln!("❌ Error saving tasks: {}", err);
        } else {
            println!("✅ Tasks saved to {}", TASKS_FILE);
        }
    }

    // Save log to file before exiting
    if session.log.is_dirty() {
        if let Err(err) = store::save_worklog(&session.log, WORKLOG_FILE) {
            eprintln!("❌ Error saving logs: {}", err);
        } else {
            println!("✅ Worklogs saved to {}", WORKLOG_FILE);
        }
    }
    // Save history
    rl.save_history(COMMAND_HISTORY_FILE)?;

    Ok(())
}
