use chrono::{Duration, NaiveDateTime, NaiveTime};

use super::work::{WORKDAYS_PER_WEEK, WORKHOURS_PER_DAY};

pub fn parse_human_duration(input: &str) -> Option<Duration> {
    let input = input.trim().to_lowercase();
    let (num_str, unit) = input.trim().split_at(input.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(input.len()));

    let value: f64 = num_str.parse().ok()?;
    let mins = match unit.trim() {
        "m" | "min" | "mins" => value,
        "h" | "hr" | "hrs" => value * 60.0,
        "d" | "day" | "days" => value * 60.0 * WORKHOURS_PER_DAY as f64,
        "w" | "week" | "weeks" => value * 60.0 * (WORKHOURS_PER_DAY * WORKDAYS_PER_WEEK) as f64,
        _ => return None,
    };

    Some(Duration::minutes(mins.round() as i64))
}

pub fn parse_human_duration_with_sign(input: &str) -> Option<(Option<i32>, Duration)> {
    let input = input.trim().to_lowercase();
    let sign = if input.starts_with('-') {
        Some(-1)
    } else if input.starts_with('+') {
        Some(1)
    } else {
        None
    };

    let duration_str = if sign.is_some() { &input[1..] } else { &input };

    let duration = parse_human_duration(duration_str)?;
    Some((sign, duration))
}

#[test]
fn test_parse_human_duration() {
    assert_eq!(parse_human_duration("1h"), Some(Duration::minutes(60)));
    assert_eq!(parse_human_duration("2d"), Some(Duration::minutes(60 * 8 * 2)));
    assert_eq!(parse_human_duration("3w"), Some(Duration::minutes(60 * 8 * 5 * 3)));
    assert_eq!(parse_human_duration("4m"), Some(Duration::minutes(4)));
    assert_eq!(parse_human_duration("5min"), Some(Duration::minutes(5)));
    assert_eq!(parse_human_duration("6mins"), Some(Duration::minutes(6)));
    assert_eq!(parse_human_duration("7.5h"), Some(Duration::minutes(450)));
    assert_eq!(parse_human_duration("4.0d"), Some(Duration::minutes(4 * 60 * 8)));
    assert_eq!(parse_human_duration("9.5w"), Some(Duration::minutes(22800)));
    assert_eq!(parse_human_duration("invalid"), None);
}

pub fn format_human_duration(duration: Duration) -> String {
    let mut total_minutes = duration.num_minutes();

    if total_minutes <= 0 {
        return "0min".to_string();
    }

    let weeks = total_minutes / (60 * WORKHOURS_PER_DAY * WORKDAYS_PER_WEEK);
    total_minutes -= weeks * (60 * WORKHOURS_PER_DAY * WORKDAYS_PER_WEEK);
    let days = total_minutes / (60 * WORKHOURS_PER_DAY);
    total_minutes -= days * (60 * WORKHOURS_PER_DAY);
    let hours = total_minutes / 60;
    total_minutes -= hours * 60;
    let minutes = total_minutes as f64 + (duration.num_seconds() % 60) as f64 / 60.0;

    let mut parts = vec![];

    if weeks > 0 {
        parts.push(format!("{}w", weeks));
    }
    if days > 0 {
        parts.push(format!("{}d", days));
    }
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }
    if minutes > 0.0 {
        parts.push(format!("{}min", minutes.ceil()));
    }

    parts.join(" ")
}

#[test]
fn test_format_human_duration() {
    assert_eq!(format_human_duration(Duration::minutes(0)), "0min");
    assert_eq!(format_human_duration(Duration::minutes(1)), "1min");
    assert_eq!(format_human_duration(Duration::minutes(60)), "1h");
    assert_eq!(format_human_duration(Duration::minutes(120)), "2h");
    assert_eq!(format_human_duration(Duration::minutes(480)), "1d");
    assert_eq!(format_human_duration(Duration::minutes(1440)), "3d");
    assert_eq!(format_human_duration(Duration::minutes(2402)), "1w 2min");
}

pub enum StopKind {
    Immediately(NaiveDateTime),
    EndsAt(NaiveDateTime),
    EndsIn(Duration),
}

pub fn parse_stop_kind(args: &[&str], now: NaiveDateTime) -> Option<StopKind> {
    match args {
        [] => Some(StopKind::EndsAt(now)),
        ["in", duration_str] => {
            let duration = parse_human_duration(duration_str)?;
            Some(StopKind::EndsIn(duration))
        }
        ["at", time_str] => {
            let time = NaiveTime::parse_from_str(time_str, "%H:%M").ok()?;
            let end_time = NaiveDateTime::new(now.date(), time);
            if end_time < now {
                return None;
            }
            Some(StopKind::EndsAt(end_time))
        }
        ["immediately"] => Some(StopKind::Immediately(now)),
        _ => None,
    }
}
