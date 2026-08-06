use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{Datelike, Duration as ChronoDuration, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{
    chat::model::{GenerateRequest, ModelUsage},
    chat::model_metadata,
    settings::ModelProvider,
    state::AppState,
};

const USAGE_DIR_NAME: &str = "usage";
const DEFAULT_LOG_LIMIT: usize = 100;
const MAX_LOG_LIMIT: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub id: String,
    pub created_at: i64,
    pub completed_at: i64,
    pub duration_ms: u64,
    #[serde(default)]
    pub first_token_ms: Option<u64>,
    pub source: String,
    pub operation: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub api_format: String,
    pub status: String,
    #[serde(default)]
    pub status_code: Option<u16>,
    pub usage_source: String,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub cached_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    pub cost_source: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatsQuery {
    #[serde(default = "default_range")]
    pub range: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub provider_search: Option<String>,
    #[serde(default)]
    pub model_search: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub missing_usage_requests: u64,
    pub provider_reported_requests: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_cost_usd: f64,
    pub average_duration_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTrendPoint {
    pub date: String,
    pub label: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroupStats {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub request_count: u64,
    pub success_count: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
    pub average_duration_ms: Option<f64>,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatsResponse {
    pub summary: UsageSummary,
    pub trend: Vec<UsageTrendPoint>,
    pub logs: Vec<UsageRecord>,
    pub provider_stats: Vec<UsageGroupStats>,
    pub model_stats: Vec<UsageGroupStats>,
    pub total_logs: usize,
    pub skipped_records: usize,
}

pub struct UsageRecordInput<'a> {
    pub provider: &'a ModelProvider,
    pub model: &'a str,
    pub source: &'a str,
    pub operation: &'a str,
    pub status: &'a str,
    pub status_code: Option<u16>,
    pub usage: Option<ModelUsage>,
    pub usage_source: &'a str,
    pub started_at: i64,
    pub duration_ms: u64,
    pub first_token_ms: Option<u64>,
    pub reasoning_effort: Option<String>,
    pub conversation_id: Option<String>,
    pub message_id: Option<String>,
    pub error_kind: Option<String>,
}

fn default_range() -> String {
    "7d".to_string()
}

pub fn usage_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir unavailable: {e}"))?
        .join(USAGE_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|e| format!("create usage dir: {e}"))?;
    Ok(dir)
}

pub fn record_model_call(state: &AppState, input: UsageRecordInput<'_>) {
    let completed_at = Local::now().timestamp();
    let usage = input.usage;
    let input_tokens = usage.as_ref().and_then(|usage| usage.input_tokens);
    let output_tokens = usage.as_ref().and_then(|usage| usage.output_tokens);
    let total_tokens = usage
        .as_ref()
        .and_then(|usage| usage.total_tokens)
        .or_else(|| {
            input_tokens
                .zip(output_tokens)
                .map(|(a, b)| a.saturating_add(b))
        });
    let cached_input_tokens = usage.as_ref().and_then(|usage| usage.cached_input_tokens);
    let cache_creation_input_tokens = usage
        .as_ref()
        .and_then(|usage| usage.cache_creation_input_tokens);
    let reasoning_tokens = usage.as_ref().and_then(|usage| usage.reasoning_tokens);
    let mut record = UsageRecord {
        id: format!("usage_{}", Uuid::new_v4()),
        created_at: input.started_at,
        completed_at,
        duration_ms: input.duration_ms,
        first_token_ms: input.first_token_ms,
        source: input.source.to_string(),
        operation: input.operation.to_string(),
        provider_id: input.provider.id.clone(),
        provider_name: input.provider.name.clone(),
        model: input.model.to_string(),
        api_format: input.provider.api_format.clone(),
        status: input.status.to_string(),
        status_code: input.status_code,
        usage_source: if usage.is_some() {
            input.usage_source.to_string()
        } else {
            "missing".to_string()
        },
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
        reasoning_tokens,
        reasoning_effort: input.reasoning_effort,
        cost_usd: None,
        cost_source: "unavailable".to_string(),
        conversation_id: input.conversation_id,
        message_id: input.message_id,
        error_kind: input.error_kind,
    };
    apply_cost(input.provider, &mut record);
    if let Err(err) = append_record(&state.usage_dir, &record) {
        eprintln!("Failed to record usage: {err}");
    }
}

/// Explicit effort sent for this request. `None` means the provider/model default was used.
pub fn reasoning_effort_for_request(request: &GenerateRequest) -> Option<String> {
    if request.options.thinking_enabled {
        request.options.thinking_level.clone()
    } else {
        Some("none".to_string())
    }
}

fn append_record(dir: &Path, record: &UsageRecord) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create usage dir: {e}"))?;
    let path = dir.join(monthly_file_name(record.created_at));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open usage file: {e}"))?;
    let line = serde_json::to_string(record).map_err(|e| format!("serialize usage: {e}"))?;
    writeln!(file, "{line}").map_err(|e| format!("write usage file: {e}"))
}

fn monthly_file_name(timestamp: i64) -> String {
    let date = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Local::now);
    format!("usage-{:04}-{:02}.jsonl", date.year(), date.month())
}

fn apply_cost(provider: &ModelProvider, record: &mut UsageRecord) {
    let Some((pricing, source)) = model_metadata::pricing_for_model(Some(provider), &record.model)
    else {
        return;
    };
    let input_price = pricing.input.unwrap_or(0.0);
    let output_price = pricing.output.unwrap_or(0.0);
    let cached_price = pricing.cached_input.unwrap_or(input_price);
    let input_tokens = record.input_tokens.unwrap_or(0);
    let output_tokens = record.output_tokens.unwrap_or(0);
    // 缓存计价口径对齐 context_estimate::anchor_total_tokens（commit 85a3056）：
    // Anthropic 的 input_tokens 是**非缓存**部分，cache_read/cache_creation 与其
    // 不相交——input 全额按原价计，cache_read 按缓存价另加（cache_creation 无独立
    // 价格字段，按原价近似）。OpenAI 系 cached 是 prompt_tokens 的子集，先扣再算。
    let cost = if record.api_format == "anthropic_messages" {
        let cache_read = record.cached_input_tokens.unwrap_or(0);
        let cache_creation = record.cache_creation_input_tokens.unwrap_or(0);
        (input_tokens as f64 * input_price
            + cache_read as f64 * cached_price
            + cache_creation as f64 * input_price
            + output_tokens as f64 * output_price)
            / 1_000_000.0
    } else {
        let cached_tokens = record.cached_input_tokens.unwrap_or(0).min(input_tokens);
        let uncached_input_tokens = input_tokens.saturating_sub(cached_tokens);
        (uncached_input_tokens as f64 * input_price
            + cached_tokens as f64 * cached_price
            + output_tokens as f64 * output_price)
            / 1_000_000.0
    };
    record.cost_usd = Some(cost);
    record.cost_source = source;
}

fn read_records(dir: &Path, start: Option<i64>) -> (Vec<UsageRecord>, usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), 0);
    };
    let mut records = Vec::new();
    let mut skipped = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if usage_file_is_before_start(&path, start) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<UsageRecord>(line) {
                Ok(record) => records.push(record),
                Err(_) => skipped = skipped.saturating_add(1),
            }
        }
    }
    (records, skipped)
}

fn usage_file_is_before_start(path: &Path, start: Option<i64>) -> bool {
    let Some(start) = start else {
        return false;
    };
    let Some(next_month_start) = usage_file_next_month_start(path) else {
        return false;
    };
    next_month_start <= start
}

fn usage_file_next_month_start(path: &Path) -> Option<i64> {
    let file_name = path.file_name()?.to_str()?;
    let stem = file_name.strip_prefix("usage-")?.strip_suffix(".jsonl")?;
    let (year, month) = stem.split_once('-')?;
    let mut year = year.parse::<i32>().ok()?;
    let mut month = month.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    if month == 12 {
        year = year.saturating_add(1);
        month = 1;
    } else {
        month += 1;
    }
    Local
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .map(|date| date.timestamp())
}

#[tauri::command]
pub fn usage_get_stats(
    state: State<'_, AppState>,
    query: Option<UsageStatsQuery>,
) -> Result<UsageStatsResponse, String> {
    let query = query.unwrap_or_default();
    let start = range_start(&query.range);
    let (records, skipped_records) = read_records(&state.usage_dir, start);
    let filtered = filter_records(records, &query);
    let total_logs = filtered.len();
    let summary = summarize(&filtered);
    let trend = build_trend(&filtered, &query.range);
    let provider_stats = group_provider_stats(&filtered);
    let model_stats = group_model_stats(&filtered);
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_LOG_LIMIT).min(MAX_LOG_LIMIT);
    let logs = filtered.into_iter().skip(offset).take(limit).collect();
    Ok(UsageStatsResponse {
        summary,
        trend,
        logs,
        provider_stats,
        model_stats,
        total_logs,
        skipped_records,
    })
}

#[tauri::command]
pub fn usage_clear(state: State<'_, AppState>) -> Result<(), String> {
    if !state.usage_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&state.usage_dir).map_err(|e| format!("read usage dir: {e}"))? {
        let entry = entry.map_err(|e| format!("read usage entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            fs::remove_file(&path).map_err(|e| format!("remove usage file: {e}"))?;
        }
    }
    Ok(())
}

fn filter_records(mut records: Vec<UsageRecord>, query: &UsageStatsQuery) -> Vec<UsageRecord> {
    let start = range_start(&query.range);
    let source = normalized_filter(query.source.as_deref());
    let status = normalized_filter(query.status.as_deref());
    let provider_search = normalized_search(query.provider_search.as_deref());
    let model_search = normalized_search(query.model_search.as_deref());
    records.retain(|record| {
        if let Some(start) = start {
            if record.created_at < start {
                return false;
            }
        }
        if let Some(source) = source.as_deref() {
            if record.source != source {
                return false;
            }
        }
        if let Some(status) = status.as_deref() {
            if status == "missing_usage" {
                if record.usage_source != "missing" {
                    return false;
                }
            } else if record.status != status {
                return false;
            }
        }
        if let Some(search) = provider_search.as_deref() {
            let haystack =
                format!("{} {}", record.provider_id, record.provider_name).to_ascii_lowercase();
            if !haystack.contains(search) {
                return false;
            }
        }
        if let Some(search) = model_search.as_deref() {
            if !record.model.to_ascii_lowercase().contains(search) {
                return false;
            }
        }
        true
    });
    records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    records
}

fn range_start(range: &str) -> Option<i64> {
    let now = Local::now().timestamp();
    match range {
        // 「当天」= 本地日历日起点（今日 00:00），随后各档为滚动窗口。
        "today" => Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|naive| Local.from_local_datetime(&naive).single())
            .map(|dt| dt.timestamp()),
        "1d" => Some(now.saturating_sub(86_400)),
        "30d" => Some(now.saturating_sub(30 * 86_400)),
        "90d" => Some(now.saturating_sub(90 * 86_400)),
        "all" => None,
        // default 7d
        _ => Some(now.saturating_sub(7 * 86_400)),
    }
}

fn normalized_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "all")
        .map(str::to_string)
}

fn normalized_search(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn summarize(records: &[UsageRecord]) -> UsageSummary {
    let mut summary = UsageSummary::default();
    let mut duration_total = 0u64;
    let mut duration_count = 0u64;
    for record in records {
        summary.total_requests = summary.total_requests.saturating_add(1);
        if record.status == "success" {
            summary.successful_requests = summary.successful_requests.saturating_add(1);
        } else {
            summary.failed_requests = summary.failed_requests.saturating_add(1);
        }
        if record.usage_source == "missing" {
            summary.missing_usage_requests = summary.missing_usage_requests.saturating_add(1);
        }
        if record.usage_source == "provider_reported" {
            summary.provider_reported_requests =
                summary.provider_reported_requests.saturating_add(1);
        }
        summary.total_tokens = summary
            .total_tokens
            .saturating_add(record_total_tokens(record));
        summary.input_tokens = summary
            .input_tokens
            .saturating_add(record_effective_input_tokens(record));
        summary.output_tokens = summary
            .output_tokens
            .saturating_add(record.output_tokens.unwrap_or(0));
        summary.cached_input_tokens = summary
            .cached_input_tokens
            .saturating_add(record.cached_input_tokens.unwrap_or(0));
        summary.cache_creation_input_tokens = summary
            .cache_creation_input_tokens
            .saturating_add(record.cache_creation_input_tokens.unwrap_or(0));
        summary.reasoning_tokens = summary
            .reasoning_tokens
            .saturating_add(record.reasoning_tokens.unwrap_or(0));
        summary.total_cost_usd += record.cost_usd.unwrap_or(0.0);
        duration_total = duration_total.saturating_add(record.duration_ms);
        duration_count = duration_count.saturating_add(1);
    }
    if duration_count > 0 {
        summary.average_duration_ms = Some(duration_total as f64 / duration_count as f64);
    }
    summary
}

/// 统一「输入 tokens」口径（对齐 context_estimate::anchor_total_tokens，commit
/// 85a3056）：Anthropic 的 input_tokens 是**非缓存**部分，全量输入 = input +
/// cache_read + cache_creation（三者不相交）；OpenAI 系 input(=prompt_tokens)
/// 已含 cached，直接用。聚合（summary/trend/分组）一律走这里，缓存命中率
/// cached/input 才对两种口径同时成立。
fn record_effective_input_tokens(record: &UsageRecord) -> u64 {
    let input = record.input_tokens.unwrap_or(0);
    if record.api_format == "anthropic_messages" {
        input
            .saturating_add(record.cached_input_tokens.unwrap_or(0))
            .saturating_add(record.cache_creation_input_tokens.unwrap_or(0))
    } else {
        input
    }
}

fn record_total_tokens(record: &UsageRecord) -> u64 {
    // Anthropic：不能用落盘的 total_tokens（= input+output，漏 cache，见
    // model_usage_from_anthropic_value）——显式加回缓存部分。
    if record.api_format == "anthropic_messages" {
        return record_effective_input_tokens(record)
            .saturating_add(record.output_tokens.unwrap_or(0));
    }
    record.total_tokens.unwrap_or_else(|| {
        record
            .input_tokens
            .unwrap_or(0)
            .saturating_add(record.output_tokens.unwrap_or(0))
    })
}

fn build_trend(records: &[UsageRecord], range: &str) -> Vec<UsageTrendPoint> {
    let mut points: BTreeMap<String, UsageTrendPoint> = BTreeMap::new();
    // 「当天」按小时分桶（今日 00:00 至当前小时），其余档位按天分桶。
    let hourly = range == "today";
    if hourly {
        let now = Local::now();
        let today = now.date_naive();
        for hour in 0..=now.hour() {
            let key = format!("{} {:02}", today.format("%Y-%m-%d"), hour);
            points.insert(
                key.clone(),
                UsageTrendPoint {
                    date: key,
                    label: format!("{:02}:00", hour),
                    ..UsageTrendPoint::default()
                },
            );
        }
    } else if let Some(days) = range_days(range) {
        let today = Local::now().date_naive();
        for offset in (0..days).rev() {
            let date = today - ChronoDuration::days(offset as i64);
            let key = date.format("%Y-%m-%d").to_string();
            points.insert(
                key.clone(),
                UsageTrendPoint {
                    date: key,
                    label: date.format("%m/%d").to_string(),
                    ..UsageTrendPoint::default()
                },
            );
        }
    }
    for record in records {
        let dt = Local
            .timestamp_opt(record.created_at, 0)
            .single()
            .unwrap_or_else(Local::now);
        let (key, label) = if hourly {
            (
                format!("{} {:02}", dt.date_naive().format("%Y-%m-%d"), dt.hour()),
                format!("{:02}:00", dt.hour()),
            )
        } else {
            let date = dt.date_naive();
            (
                date.format("%Y-%m-%d").to_string(),
                date.format("%m/%d").to_string(),
            )
        };
        let point = points
            .entry(key.clone())
            .or_insert_with(|| UsageTrendPoint {
                date: key,
                label,
                ..UsageTrendPoint::default()
            });
        point.requests = point.requests.saturating_add(1);
        point.total_tokens = point
            .total_tokens
            .saturating_add(record_total_tokens(record));
        point.input_tokens = point
            .input_tokens
            .saturating_add(record_effective_input_tokens(record));
        point.output_tokens = point
            .output_tokens
            .saturating_add(record.output_tokens.unwrap_or(0));
        point.cached_input_tokens = point
            .cached_input_tokens
            .saturating_add(record.cached_input_tokens.unwrap_or(0));
        point.cache_creation_input_tokens = point
            .cache_creation_input_tokens
            .saturating_add(record.cache_creation_input_tokens.unwrap_or(0));
        point.cost_usd += record.cost_usd.unwrap_or(0.0);
    }
    points.into_values().collect()
}

fn range_days(range: &str) -> Option<usize> {
    match range {
        "today" | "1d" => Some(1),
        "30d" => Some(30),
        "90d" => Some(90),
        "all" => None,
        // default 7d
        _ => Some(7),
    }
}

fn group_provider_stats(records: &[UsageRecord]) -> Vec<UsageGroupStats> {
    let mut groups: BTreeMap<String, UsageGroupStatsAccumulator> = BTreeMap::new();
    for record in records {
        let entry = groups
            .entry(record.provider_id.clone())
            .or_insert_with(|| UsageGroupStatsAccumulator::new_provider(record));
        entry.add(record);
    }
    finalize_groups(groups)
}

fn group_model_stats(records: &[UsageRecord]) -> Vec<UsageGroupStats> {
    let mut groups: BTreeMap<String, UsageGroupStatsAccumulator> = BTreeMap::new();
    for record in records {
        let key = format!("{}::{}", record.provider_id, record.model);
        let entry = groups
            .entry(key)
            .or_insert_with(|| UsageGroupStatsAccumulator::new_model(record));
        entry.add(record);
    }
    finalize_groups(groups)
}

#[derive(Debug, Clone)]
struct UsageGroupStatsAccumulator {
    stats: UsageGroupStats,
    duration_total: u64,
    duration_count: u64,
}

impl UsageGroupStatsAccumulator {
    fn new_provider(record: &UsageRecord) -> Self {
        Self {
            stats: UsageGroupStats {
                id: record.provider_id.clone(),
                label: if record.provider_name.trim().is_empty() {
                    record.provider_id.clone()
                } else {
                    record.provider_name.clone()
                },
                provider_id: Some(record.provider_id.clone()),
                provider_name: Some(record.provider_name.clone()),
                ..UsageGroupStats::default()
            },
            duration_total: 0,
            duration_count: 0,
        }
    }

    fn new_model(record: &UsageRecord) -> Self {
        Self {
            stats: UsageGroupStats {
                id: format!("{}::{}", record.provider_id, record.model),
                label: record.model.clone(),
                provider_id: Some(record.provider_id.clone()),
                provider_name: Some(record.provider_name.clone()),
                model: Some(record.model.clone()),
                ..UsageGroupStats::default()
            },
            duration_total: 0,
            duration_count: 0,
        }
    }

    fn add(&mut self, record: &UsageRecord) {
        self.stats.request_count = self.stats.request_count.saturating_add(1);
        if record.status == "success" {
            self.stats.success_count = self.stats.success_count.saturating_add(1);
        }
        self.stats.total_tokens = self
            .stats
            .total_tokens
            .saturating_add(record_total_tokens(record));
        self.stats.input_tokens = self
            .stats
            .input_tokens
            .saturating_add(record_effective_input_tokens(record));
        self.stats.output_tokens = self
            .stats
            .output_tokens
            .saturating_add(record.output_tokens.unwrap_or(0));
        self.stats.cached_input_tokens = self
            .stats
            .cached_input_tokens
            .saturating_add(record.cached_input_tokens.unwrap_or(0));
        self.stats.cache_creation_input_tokens = self
            .stats
            .cache_creation_input_tokens
            .saturating_add(record.cache_creation_input_tokens.unwrap_or(0));
        self.stats.cost_usd += record.cost_usd.unwrap_or(0.0);
        self.duration_total = self.duration_total.saturating_add(record.duration_ms);
        self.duration_count = self.duration_count.saturating_add(1);
        self.stats.last_used_at = Some(
            self.stats
                .last_used_at
                .unwrap_or(record.created_at)
                .max(record.created_at),
        );
    }
}

fn finalize_groups(groups: BTreeMap<String, UsageGroupStatsAccumulator>) -> Vec<UsageGroupStats> {
    let mut stats = groups
        .into_values()
        .map(|mut group| {
            if group.duration_count > 0 {
                group.stats.average_duration_ms =
                    Some(group.duration_total as f64 / group.duration_count as f64);
            }
            group.stats
        })
        .collect::<Vec<_>>();
    stats.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));
    stats
}

pub fn model_usage_from_openai_value(value: &Value) -> Option<ModelUsage> {
    let usage = value.get("usage")?;
    let prompt_details = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"));
    let completion_details = usage
        .get("completion_tokens_details")
        .or_else(|| usage.get("output_tokens_details"));
    Some(ModelUsage {
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("input_tokens").and_then(Value::as_u64)),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .or_else(|| usage.get("output_tokens").and_then(Value::as_u64)),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        cached_input_tokens: prompt_details
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64),
        cache_creation_input_tokens: prompt_details
            .and_then(|details| details.get("cache_creation_tokens"))
            .and_then(Value::as_u64),
        reasoning_tokens: completion_details
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64),
        // 内置 provider 路径：窗口来自 model_metadata，不由响应携带。
        context_window_tokens: None,
    })
}

pub fn model_usage_from_anthropic_value(value: &Value) -> Option<ModelUsage> {
    let usage = value.get("usage")?;
    let input = usage.get("input_tokens").and_then(Value::as_u64);
    let output = usage.get("output_tokens").and_then(Value::as_u64);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    let cache_read = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    Some(ModelUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input.zip(output).map(|(a, b)| a.saturating_add(b)),
        cached_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
        reasoning_tokens: None,
        // 内置 provider 路径：窗口来自 model_metadata，不由响应携带。
        context_window_tokens: None,
    })
}

pub fn model_usage_from_stream_value(value: &Value) -> Option<ModelUsage> {
    model_usage_from_openai_value(value).or_else(|| model_usage_from_anthropic_value(value))
}

pub fn chat_usage_source_for_label(label: &str) -> String {
    let lower = label.to_ascii_lowercase();
    if lower.contains("title") {
        "chat_title_summary".to_string()
    } else if lower.contains("compression") {
        "chat_compression".to_string()
    } else if lower.contains("auxiliary vision") {
        "chat_aux_vision".to_string()
    } else if lower.contains("image generation") {
        "chat_image_generation".to_string()
    } else {
        "chat".to_string()
    }
}

pub fn operation_from_label(label: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = false;
    for ch in label.chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_sep = false;
        } else if !last_was_sep && !out.is_empty() {
            out.push('_');
            last_was_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "model_call".to_string()
    } else {
        out
    }
}

pub fn error_kind_from_message(message: &str) -> String {
    if let Some(code) = crate::api::extract_status_code(message) {
        return format!("http_{code}");
    }
    let lower = message.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        "timeout".to_string()
    } else if lower.contains("cancelled") || lower.contains("canceled") {
        "cancelled".to_string()
    } else if lower.contains("parse") || lower.contains("json") {
        "parse_error".to_string()
    } else if lower.contains("read body") || lower.contains("stream") {
        "stream_error".to_string()
    } else {
        "request_error".to_string()
    }
}

/// 失败记录的 usage status：错误文案命中取消语义时记 `"cancelled"`，否则 `"error"`。
/// 四个模型适配器的 `record_usage_failure` 签名只有错误字符串（不带 `ModelError`），
/// 故在共享层按与 `error_kind_from_message` 同一口径判定，避免改四处签名。
pub fn failure_status_from_message(message: &str) -> &'static str {
    if error_kind_from_message(message) == "cancelled" {
        "cancelled"
    } else {
        "error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_cached_and_reasoning_usage() {
        let value = serde_json::json!({
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": { "cached_tokens": 80 },
                "completion_tokens_details": { "reasoning_tokens": 5 }
            }
        });
        let usage = model_usage_from_openai_value(&value).expect("usage");
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cached_input_tokens, Some(80));
        assert_eq!(usage.reasoning_tokens, Some(5));
    }

    #[test]
    fn filter_missing_usage_status() {
        let mut record = UsageRecord {
            id: "usage_test".to_string(),
            created_at: Local::now().timestamp(),
            completed_at: Local::now().timestamp(),
            duration_ms: 1,
            first_token_ms: None,
            source: "chat".to_string(),
            operation: "plain".to_string(),
            provider_id: "p".to_string(),
            provider_name: "Provider".to_string(),
            model: "model".to_string(),
            api_format: "openai_chat".to_string(),
            status: "success".to_string(),
            status_code: Some(200),
            usage_source: "missing".to_string(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_tokens: None,
            reasoning_effort: None,
            cost_usd: None,
            cost_source: "unavailable".to_string(),
            conversation_id: None,
            message_id: None,
            error_kind: None,
        };
        let query = UsageStatsQuery {
            status: Some("missing_usage".to_string()),
            ..UsageStatsQuery::default()
        };
        assert_eq!(filter_records(vec![record.clone()], &query).len(), 1);
        record.usage_source = "provider_reported".to_string();
        assert!(filter_records(vec![record], &query).is_empty());
    }

    fn base_record(api_format: &str) -> UsageRecord {
        UsageRecord {
            id: "usage_test".to_string(),
            created_at: Local::now().timestamp(),
            completed_at: Local::now().timestamp(),
            duration_ms: 1,
            first_token_ms: None,
            source: "chat".to_string(),
            operation: "plain".to_string(),
            provider_id: "p".to_string(),
            provider_name: "Provider".to_string(),
            model: "model".to_string(),
            api_format: api_format.to_string(),
            status: "success".to_string(),
            status_code: Some(200),
            usage_source: "provider_reported".to_string(),
            input_tokens: Some(1_000),
            output_tokens: Some(100),
            total_tokens: Some(1_100),
            cached_input_tokens: Some(800),
            cache_creation_input_tokens: Some(50),
            reasoning_tokens: None,
            reasoning_effort: None,
            cost_usd: None,
            cost_source: "unavailable".to_string(),
            conversation_id: None,
            message_id: None,
            error_kind: None,
        }
    }

    /// 口径对齐 context_estimate::anchor_total_tokens（85a3056）：Anthropic 的
    /// input 是非缓存部分，全量要显式加 cache_read + cache_creation；OpenAI 的
    /// input 已含 cached，直接用。
    #[test]
    fn effective_input_and_total_disambiguate_by_api_format() {
        let anthropic = base_record("anthropic_messages");
        // 1000(非缓存) + 800(cache_read) + 50(cache_creation) = 1850
        assert_eq!(record_effective_input_tokens(&anthropic), 1_850);
        // 全量 = 1850 + 100(out)，不能用漏 cache 的落盘 total_tokens(1100)。
        assert_eq!(record_total_tokens(&anthropic), 1_950);

        let openai = base_record("openai_chat");
        assert_eq!(record_effective_input_tokens(&openai), 1_000);
        assert_eq!(record_total_tokens(&openai), 1_100); // 落盘 total 优先
    }

    #[test]
    fn usage_record_new_fields_are_backward_compatible_and_camel_case() {
        let mut value = serde_json::to_value(base_record("openai_chat")).expect("serialize");
        let object = value.as_object_mut().expect("record object");
        object.remove("firstTokenMs");
        object.remove("reasoningEffort");

        let old_record: UsageRecord = serde_json::from_value(value).expect("old record");
        assert_eq!(old_record.first_token_ms, None);
        assert_eq!(old_record.reasoning_effort, None);

        let mut current = base_record("openai_responses");
        current.first_token_ms = Some(1_250);
        current.reasoning_effort = Some("high".to_string());
        let serialized = serde_json::to_value(current).expect("serialize current record");
        assert_eq!(serialized["firstTokenMs"], 1_250);
        assert_eq!(serialized["reasoningEffort"], "high");
    }

    #[test]
    fn skips_usage_file_before_range_start() {
        let start = Local
            .with_ymd_and_hms(2026, 6, 8, 0, 0, 0)
            .single()
            .expect("valid date")
            .timestamp();

        assert!(usage_file_is_before_start(
            Path::new("usage-2026-05.jsonl"),
            Some(start)
        ));
        assert!(!usage_file_is_before_start(
            Path::new("usage-2026-06.jsonl"),
            Some(start)
        ));
        assert!(!usage_file_is_before_start(
            Path::new("usage-2026-07.jsonl"),
            Some(start)
        ));
        assert!(!usage_file_is_before_start(
            Path::new("other.jsonl"),
            Some(start)
        ));
        assert!(!usage_file_is_before_start(
            Path::new("usage-2026-05.jsonl"),
            None
        ));
    }
}
