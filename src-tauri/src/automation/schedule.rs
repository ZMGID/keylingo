use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{Datelike, Days, Local, NaiveTime};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

use super::runner;
use super::storage;
use super::types::{Automation, RunOrigin};

/// 无论下次触发多远，至少每小时醒一次自愈（时钟跳变 / 漏 poke 的兜底）。
const MAX_SLEEP: Duration = Duration::from_secs(3600);
const MIN_SLEEP: Duration = Duration::from_secs(1);

static WAKE: OnceLock<Notify> = OnceLock::new();

fn wake() -> &'static Notify {
    WAKE.get_or_init(Notify::new)
}

/// 自动化库有变更（保存/启停/删除/导入）时立即唤醒调度器重算下次触发。
pub(crate) fn poke() {
    wake().notify_one();
}

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
        loop {
            let sleep_for = if app.try_state::<crate::state::AppState>().is_none() {
                Duration::from_secs(5)
            } else {
                tick_once(&app)
            };
            tokio::select! {
                _ = tokio::time::sleep(sleep_for) => {}
                _ = wake().notified() => {}
            }
        }
    });
}

/// 每个 schedule 触发器解析出来的调度参数。
struct ScheduleSpec {
    id: String,
    kind: String,
    hour: u32,
    minute: u32,
    interval_minutes: i64,
}

fn collect_specs(automations: &[Automation]) -> Vec<ScheduleSpec> {
    let mut specs = Vec::new();
    for automation in automations {
        if !automation.enabled {
            continue;
        }
        let Some(trigger) = automation
            .nodes
            .iter()
            .find(|node| node.node_type == "trigger.schedule")
        else {
            continue;
        };
        let schedule = trigger.data.get("schedule").cloned().unwrap_or_default();
        specs.push(ScheduleSpec {
            id: automation.id.clone(),
            kind: schedule
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("daily")
                .to_string(),
            hour: schedule.get("hour").and_then(|v| v.as_u64()).unwrap_or(9) as u32,
            minute: schedule.get("minute").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            interval_minutes: schedule
                .get("intervalMinutes")
                .and_then(|v| v.as_u64())
                .unwrap_or(60)
                .max(1) as i64,
        });
    }
    specs
}

/// 跑一轮到期检查，返回距下次该醒来的时长（无启用 schedule 时为 MAX_SLEEP）。
fn tick_once(app: &AppHandle) -> Duration {
    let Ok(automations) = storage::list_full(app) else {
        return MAX_SLEEP;
    };
    let specs = collect_specs(&automations);
    if specs.is_empty() {
        return MAX_SLEEP;
    }
    let now = Local::now();
    let mut state = load_state(app);
    let mut dirty = false;
    let mut next_wake: Option<chrono::DateTime<Local>> = None;
    for spec in specs {
        let mut last = state
            .last_fired
            .get(&spec.id)
            .and_then(|iso| chrono::DateTime::parse_from_rfc3339(iso).ok())
            .map(|dt| dt.with_timezone(&Local));
        match is_due(
            &spec.kind,
            spec.hour,
            spec.minute,
            spec.interval_minutes,
            last,
            now,
        ) {
            Due::Arm => {
                state.last_fired.insert(spec.id.clone(), now.to_rfc3339());
                last = Some(now);
                dirty = true;
            }
            Due::Fire => {
                state.last_fired.insert(spec.id.clone(), now.to_rfc3339());
                last = Some(now);
                dirty = true;
                let app = app.clone();
                let id = spec.id.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = runner::enqueue(app, id, RunOrigin::Schedule, None, None) {
                        eprintln!("automation schedule: {err}");
                    }
                });
            }
            Due::Wait => {}
        }
        if let Some(due) = next_fire(&spec.kind, spec.hour, spec.minute, spec.interval_minutes, last, now)
        {
            next_wake = Some(match next_wake {
                Some(cur) if cur <= due => cur,
                _ => due,
            });
        }
    }
    if dirty {
        save_state(app, &state);
    }
    let Some(next_wake) = next_wake else {
        return MAX_SLEEP;
    };
    // +1s 余量，确保醒来时已经落在触发窗口内。
    match (next_wake - now).to_std() {
        Ok(until) => (until + Duration::from_secs(1)).clamp(MIN_SLEEP, MAX_SLEEP),
        Err(_) => MIN_SLEEP,
    }
}

/// 本轮 is_due 处理完后，这条 schedule 下一次可能触发的时刻。
fn next_fire(
    kind: &str,
    hour: u32,
    minute: u32,
    interval_minutes: i64,
    last: Option<chrono::DateTime<Local>>,
    now: chrono::DateTime<Local>,
) -> Option<chrono::DateTime<Local>> {
    match kind {
        "interval" => Some(last.unwrap_or(now) + chrono::Duration::minutes(interval_minutes)),
        _ => {
            let weekdays_only = kind == "weekdays";
            let time = NaiveTime::from_hms_opt(hour.min(23), minute.min(59), 0)?;
            for days in 0..8u64 {
                let date = now.date_naive().checked_add_days(Days::new(days))?;
                if weekdays_only && date.weekday().number_from_monday() > 5 {
                    continue;
                }
                let Some(candidate) = date.and_time(time).and_local_timezone(Local).single()
                else {
                    continue;
                };
                if candidate > now {
                    return Some(candidate);
                }
            }
            None
        }
    }
}

pub(crate) fn forget(app: &AppHandle, id: &str) {
    let mut state = load_state(app);
    if state.last_fired.remove(id).is_some() {
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
    fn next_fire_matches_schedule_kind() {
        let now = at(9, 30);
        assert_eq!(
            next_fire("interval", 0, 0, 60, Some(at(9, 0)), now),
            Some(at(10, 0))
        );
        assert_eq!(next_fire("daily", 10, 0, 60, None, now), Some(at(10, 0)));
        let tomorrow = next_fire("daily", 9, 0, 60, None, now).expect("next daily");
        assert_eq!(
            tomorrow.date_naive(),
            now.date_naive().succ_opt().expect("next day")
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
