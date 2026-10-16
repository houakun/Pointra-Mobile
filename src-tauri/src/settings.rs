//! 手机端本地设置：配对信息 / 每日上限 / 发音偏好。
//!
//! 单独存成 `settings.json`（而不是塞进某个大配置），
//! 避免「改一个字段把别的字段打回默认值」。

use crate::session::DailyGoals;
use crate::sync_client::Pairing;
use serde::{Deserialize, Serialize};
use std::path::Path;

fn default_accent() -> String {
    "us".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub pairing: Option<Pairing>,
    #[serde(default)]
    pub goals: DailyGoals,
    /// 发音偏好：`us` / `uk`
    #[serde(default = "default_accent")]
    pub audio_accent: String,
    /// 上次同步完成时间（毫秒）
    #[serde(default)]
    pub last_sync_at: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pairing: None,
            goals: DailyGoals::default(),
            audio_accent: default_accent(),
            last_sync_at: None,
        }
    }
}

/// 读设置。文件不存在或损坏时回退到默认值（不报错，保证 App 一定能启动）。
pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 原子写：先写 `.tmp` 再 rename，避免断电/杀进程留下半截 JSON。
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建设置目录失败: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(settings).map_err(|e| format!("序列化设置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写入设置失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("保存设置失败: {e}"))?;
    Ok(())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let path = std::env::temp_dir().join("pointra-mobile-nope/settings.json");
        let s = load(&path);
        assert!(s.pairing.is_none());
        assert_eq!(s.goals.new_per_day, 20);
        assert_eq!(s.audio_accent, "us");
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("pointra-mobile-settings-{}", now_ms()));
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.audio_accent = "uk".into();
        s.goals.review_per_day = 42;
        save(&path, &s).unwrap();

        let back = load(&path);
        assert_eq!(back.audio_accent, "uk");
        assert_eq!(back.goals.review_per_day, 42);
        // 清理本轮生成的临时目录（单文件，非批量删除）
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    /// 老版本 settings.json 只有部分字段时，缺失字段走默认值而不是整份解析失败。
    #[test]
    fn partial_json_keeps_defaults_for_missing_fields() {
        let dir = std::env::temp_dir().join(format!("pointra-mobile-partial-{}", now_ms()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"audio_accent":"uk"}"#).unwrap();

        let s = load(&path);
        assert_eq!(s.audio_accent, "uk");
        assert_eq!(s.goals.new_per_day, 20);
        assert!(s.pairing.is_none());

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
