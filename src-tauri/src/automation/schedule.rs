use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveTime};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::runner;
use super::storage;
use super::types::RunOrigin;

const TICK: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleStateFile {
    last_fired: HashMap<String, String>,
}

fn state_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = storage::automations_dir(app)?.join("meta");
    fs::create_dir_all(&dir).map_err(|err| format!("create schedule meta dir failed: {err}"))?;
    Ok(dir.join("schedule.json"))
}

fn load_state(app: &AppHandle) -> ScheduleStateFile {
    let Ok(path) = state_path(app) else {
        return ScheduleStateFile::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_state(app: &AppHandle, state: &ScheduleStateFile) {
    let Ok(path) = state_path(app) else {
        return;
    };
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = fs::write(path, json);
    }
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            if app.try_state::<crate::state::AppState>().is_none() {
                continue;
            }
            tick_once(&app);
        }
    });
}

fn tick_once(app: &AppHandle) {
    let Ok(metas) = storage::list(app) else {
        return;
    };
    let now = Local::now();
    let mut state = load_state(app);
    let mut dirty = false;
    for meta in metas {
        if !meta.enabled {
            continue;
        }
        let Ok(automation) = storage::get(app, &meta.id) else {
            continue;
        };
        let Some(trigger) = automation
            .nodes
            .iter()
            .find(|node| node.node_type == "trigger.schedule")
        else {
            continue;
        };
        let schedule = trigger.data.get("schedule").cloned().unwrap_or_default();
        let kind = schedule
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("daily");
        let hour = schedule.get("hour").and_then(|v| v.as_u64()).unwrap_or(9) as u32;
        let minute = schedule.get("minute").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let interval_minutes = schedule
            .get("intervalMinutes")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .max(1) as i64;
        let last = state
            .last_fired
            .get(&meta.id)
            .and_then(|iso| chrono::DateTime::parse_from_rfc3339(iso).ok())
            .map(|dt| dt.with_timezone(&Local));
        match is_due(kind, hour, minute, interval_minutes, last, now) {
            Due::Arm => {
                state
                    .last_fired
                    .insert(meta.id.clone(), now.to_rfc3339());
                dirty = true;
            }
            Due::Fire => {
                state
                    .last_fired
                    .insert(meta.id.clone(), now.to_rfc3339());
                dirty = true;
                let app = app.clone();
                let id = meta.id.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = runner::enqueue(app, id, RunOrigin::Schedule, None, None) {
                        eprintln!("automation schedule: {err}");
                    }
                });
            }
            Due::Wait => {}
        }
    }
    if dirty {
        save_state(app, &state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    Wait,
    Arm,
    Fire,
}

fn is_due(
    kind: &str,
    hour: u32,
    minute: u32,
    interval_minutes: i64,
    last: Option<chrono::DateTime<Local>>,
    now: chrono::DateTime<Local>,
) -> Due {
    match kind {
        "interval" => {
            let Some(last) = last else {
                return Due::Arm;
            };
            if now.signed_duration_since(last) >= chrono::Duration::minutes(interval_minutes) {
                Due::Fire
            } else {
                Due::Wait
            }
        }
        "weekdays" if now.weekday().number_from_monday() > 5 => Due::Wait,
        _ => {
            let Some(naive) = NaiveTime::from_hms_opt(hour.min(23), minute.min(59), 0) else {
                return Due::Wait;
            };
            let Some(target) = now.date_naive().and_time(naive).and_local_timezone(Local).single()
            else {
                return Due::Wait;
            };
            if now < target {
                return Due::Wait;
            }
            let window_end = target + chrono::Duration::minutes(2);
            if now >= window_end {
                return Due::Wait;
            }
            match last {
                Some(last) if last.date_naive() == now.date_naive() => Due::Wait,
                _ => Due::Fire,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 31, hour, minute, 0)
            .single()
            .expect("local time")
    }

    #[test]
    fn interval_arms_then_fires() {
        let t0 = at(9, 0);
        assert_eq!(is_due("interval", 9, 0, 60, None, t0), Due::Arm);
        assert_eq!(
            is_due("interval", 9, 0, 60, Some(t0), at(9, 30)),
            Due::Wait
        );
        assert_eq!(
            is_due("interval", 9, 0, 60, Some(t0), at(10, 0)),
            Due::Fire
        );
    }

    #[test]
    fn daily_fires_inside_two_minute_window() {
        let now = at(9, 0);
        assert_eq!(is_due("daily", 9, 0, 60, None, now), Due::Fire);
        assert_eq!(is_due("daily", 9, 0, 60, Some(now), at(9, 1)), Due::Wait);
        assert_eq!(is_due("daily", 9, 0, 60, None, at(9, 5)), Due::Wait);
        assert_eq!(is_due("daily", 9, 0, 60, None, at(8, 59)), Due::Wait);
    }
}
