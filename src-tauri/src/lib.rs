//! Pointra Mobile 数据层与 Android 壳。
//!
//! 数据/算法层（`db` / `sm2` / `session` / `browse` / `sync_client`）刻意**不依赖 Tauri**：
//! 它们可以在桌面上用 `cargo test` 全量验证。Tauri 只负责把这一层暴露成命令。
//!
//! 与 PC 端的契约（见 `docs/01-需求与架构设计.md`）：
//! - PC 是唯一数据源，手机端只做「增量拉取 → 落库 → 本地复习」；
//! - 重复收到同一条记录时，**只能刷新词义，不能重置学习进度**；
//! - `seq` 是全局唯一且单调递增的同步游标。

pub mod browse;
pub mod commands;
pub mod db;
pub mod session;
pub mod settings;
pub mod sm2;
pub mod sync_client;
pub mod sync_events;

/// Android 的入口点。`tauri-build` 会在移动端构建时置上 `mobile` cfg。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    commands::run()
}
