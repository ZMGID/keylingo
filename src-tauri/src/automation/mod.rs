//! 本机自动化：用户编排的触发器 → 动作图。
//!
//! 分层（不要把跑图塞进存储，也不要把画布 / 执行器塞进 Chat 或 Hooks）：
//! - [`types`]：schema v1
//! - [`storage`]：`{app_data}/automations/{id}.json`
//! - [`commands`]：Tauri IPC
//!
//! 和聊天生命周期 Hooks 分家：Hooks 是对话旁路观察；自动化是用户意图的主路径。
//! 执行器（runner）与调度（schedule）另开模块。

mod storage;
mod types;
pub(crate) mod commands;

pub use types::{Automation, AutomationMeta, SCHEMA_VERSION};
