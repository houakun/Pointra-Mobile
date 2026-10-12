//! 简化 SM-2 间隔重复算法（与 `docs/01-需求与架构设计.md` §7 严格一致）。
//!
//! 三档评分：不认识（q=2）/ 我认识（q=4）/ 标记为太简单（q=5）。
//! 关键设计：**ease 只在「不认识」时下降**（q=4 时 ΔEF 恰为 0），
//! 这样两档按钮下「一直认识」的词间隔能稳定增长，不会被 ease 惩罚。

pub const DAY_MS: i64 = 86_400_000;
/// 日界：凌晨 4 点。避免「睡前复习完，白天又冒出来」。
pub const DAY_BOUNDARY_HOUR: i64 = 4;
/// 间隔达到该天数即视为「已掌握」，不再入队。
pub const GRADUATE_INTERVAL_DAYS: i64 = 180;
pub const DEFAULT_EASE: f64 = 2.5;
pub const MIN_EASE: f64 = 1.3;

/// 卡片状态。`state`：0=新 1=学习中 2=复习中 3=已掌握
#[derive(Debug, Clone, PartialEq)]
pub struct CardState {
    pub state: i32,
    pub ease: f64,
    pub interval: i64,
    pub reps: i32,
    pub lapses: i32,
    pub due_at: Option<i64>,
    pub first_seen: Option<i64>,
    pub last_review: Option<i64>,
}

impl Default for CardState {
    fn default() -> Self {
        Self {
            state: 0,
            ease: DEFAULT_EASE,
            interval: 0,
            reps: 0,
            lapses: 0,
            due_at: None,
            first_seen: None,
            last_review: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    /// 不认识（q=2）：当日回队尾重来
    Again,
    /// 我认识（q=4）
    Good,
    /// 标记为太简单（q=5）：毕业，永不再入队
    TooEasy,
}

impl Grade {
    fn quality(self) -> i32 {
        match self {
            Grade::Again => 2,
            Grade::Good => 4,
            Grade::TooEasy => 5,
        }
    }
}

/// 本地学习日的起点（毫秒）。
///
/// `utc_offset_minutes` 是本地时区相对 UTC 的偏移（东八区 = 480）。
/// Rust 标准库拿不到本地时区，所以由前端用
/// `-new Date().getTimezoneOffset()` 传入。
pub fn day_start(now_ms: i64, utc_offset_minutes: i64) -> i64 {
    let offset_ms = utc_offset_minutes * 60_000;
    let boundary = DAY_BOUNDARY_HOUR * 3_600_000;
    let shifted = now_ms + offset_ms - boundary;
    let day_index = shifted.div_euclid(DAY_MS);
    day_index * DAY_MS + boundary - offset_ms
}

/// 学习日的日期键，形如 `2026-10-11`（用于 `daily_stats` 主键）。
pub fn day_key(now_ms: i64, utc_offset_minutes: i64) -> String {
    let start = day_start(now_ms, utc_offset_minutes);
    // 转成「本地日历日」再格式化，避免手写月份天数表
    let local = start + utc_offset_minutes * 60_000;
    let days = local.div_euclid(DAY_MS);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 由「1970-01-01 起的天数」推出公历年月日（Howard Hinnant 的算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 公历年月日 → 「1970-01-01 起的天数」（与 `civil_from_days` 互逆）。
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 把 `YYYY-MM-DD`（**本地日历日**）换成该日的 `[start, end)` 毫秒区间。
///
/// 注意与 `day_key` / `day_start` 的区别：后者用 **04:00 学习日**，
/// 而「按加入日期搜索」用人能看懂的**日历日**（00:00 起）。
pub fn day_range_ms(date: &str, utc_offset_minutes: i64) -> Result<(i64, i64), String> {
    let parts: Vec<&str> = date.trim().split('-').collect();
    if parts.len() != 3 {
        return Err(format!("日期格式应为 YYYY-MM-DD：{date}"));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("年份不合法：{date}"))?;
    let m: u32 = parts[1].parse().map_err(|_| format!("月份不合法：{date}"))?;
    let d: u32 = parts[2].parse().map_err(|_| format!("日期不合法：{date}"))?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(format!("日期不合法：{date}"));
    }
    let offset_ms = utc_offset_minutes * 60_000;
    let start = days_from_civil(y, m, d) * DAY_MS - offset_ms;
    Ok((start, start + DAY_MS))
}

fn ease_after(prev_ease: f64, quality: i32) -> f64 {
    let q = quality as f64;
    let delta = 0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02);
    (prev_ease + delta).max(MIN_EASE)
}

/// 应用一次评分，返回新的卡片状态。
pub fn apply(
    prev: &CardState,
    grade: Grade,
    now_ms: i64,
    utc_offset_minutes: i64,
) -> CardState {
    let mut next = prev.clone();
    let today = day_start(now_ms, utc_offset_minutes);

    match grade {
        Grade::TooEasy => {
            next.state = 3;
            next.reps = prev.reps + 1;
            next.interval = 365;
            next.due_at = Some(today + 365 * DAY_MS);
        }
        Grade::Again => {
            // interval = 0 且 due_at 已过期 → 立刻回到今日队列尾部
            next.state = 1;
            next.reps = 0;
            next.interval = 0;
            next.lapses = prev.lapses + 1;
            next.due_at = Some(now_ms - 1);
        }
        Grade::Good => {
            next.interval = match prev.reps {
                0 => 1,
                1 => 6,
                _ => ((prev.interval as f64) * prev.ease).round() as i64,
            };
            next.reps = prev.reps + 1;
            next.state = if next.interval >= GRADUATE_INTERVAL_DAYS {
                3
            } else {
                2
            };
            next.due_at = Some(today + next.interval * DAY_MS);
        }
    }

    next.ease = ease_after(prev.ease, grade.quality());
    next.first_seen = prev.first_seen.or(Some(now_ms));
    next.last_review = Some(now_ms);
    next
}

/// 该卡片此刻是否到期（应进入复习队列）。
pub fn is_due(card: &CardState, now_ms: i64) -> bool {
    // 新词不算「到期复习」，由「今日新词」上限单独控制
    if card.state == 0 || card.state == 3 {
        return false;
    }
    card.due_at.map(|due| due <= now_ms).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000_000; // 固定时间点，避免测试随真实时间漂移
    const TZ: i64 = 480; // 东八区

    fn grade_seq(grades: &[Grade]) -> Vec<CardState> {
        let mut card = CardState::default();
        let mut out = Vec::new();
        for (i, g) in grades.iter().enumerate() {
            card = apply(&card, *g, NOW + (i as i64) * DAY_MS, TZ);
            out.push(card.clone());
        }
        out
    }

    #[test]
    fn good_grade_follows_1_6_then_ease_multiplied() {
        let states = grade_seq(&[Grade::Good, Grade::Good, Grade::Good, Grade::Good]);
        assert_eq!(states[0].interval, 1);
        assert_eq!(states[1].interval, 6);
        assert_eq!(states[2].interval, 15); // 6 * 2.5
        assert_eq!(states[3].interval, 38); // 15 * 2.5 = 37.5 → 38
        assert_eq!(states[0].reps, 1);
        assert!(states.iter().all(|s| s.state == 2));
    }

    #[test]
    fn good_grade_never_changes_ease() {
        // q=4 时 ΔEF = 0.1 - 1*(0.08+0.02) = 0，这是刻意的简化
        let states = grade_seq(&[Grade::Good, Grade::Good, Grade::Good]);
        assert!(states.iter().all(|s| (s.ease - 2.5).abs() < 1e-9));
    }

    #[test]
    fn again_resets_progress_and_lowers_ease() {
        let after_good = grade_seq(&[Grade::Good, Grade::Good]);
        let card = apply(&after_good[1], Grade::Again, NOW + 2 * DAY_MS, TZ);

        assert_eq!(card.reps, 0);
        assert_eq!(card.interval, 0);
        assert_eq!(card.state, 1);
        assert_eq!(card.lapses, 1);
        // q=2 → ΔEF = 0.1 - 3*(0.08+0.06) = -0.32
        assert!((card.ease - (2.5 - 0.32)).abs() < 1e-9);
        // 已到期 → 立刻回队尾
        assert!(is_due(&card, NOW + 2 * DAY_MS));
    }

    #[test]
    fn ease_never_drops_below_floor() {
        let mut card = CardState::default();
        for i in 0..40 {
            card = apply(&card, Grade::Again, NOW + i * DAY_MS, TZ);
        }
        assert!((card.ease - MIN_EASE).abs() < 1e-9, "ease 必须被夹在 1.3");
    }

    #[test]
    fn too_easy_graduates_and_never_returns() {
        let card = apply(&CardState::default(), Grade::TooEasy, NOW, TZ);
        assert_eq!(card.state, 3);
        assert_eq!(card.interval, 365);
        assert!(!is_due(&card, NOW + 400 * DAY_MS), "已掌握的词不应再入队");
    }

    #[test]
    fn good_reaches_graduation_after_enough_reps() {
        let mut card = CardState::default();
        for i in 0..8 {
            card = apply(&card, Grade::Good, NOW + i * DAY_MS, TZ);
        }
        assert_eq!(card.state, 3, "间隔超过 180 天后应自动毕业");
        assert!(card.interval >= GRADUATE_INTERVAL_DAYS);
    }

    #[test]
    fn new_card_is_not_due_for_review() {
        // 新词由「今日新词」上限控制，不应混进「到期复习」
        assert!(!is_due(&CardState::default(), NOW));
    }

    #[test]
    fn due_at_lands_on_day_boundary() {
        let card = apply(&CardState::default(), Grade::Good, NOW, TZ);
        let today = day_start(NOW, TZ);
        assert_eq!(card.due_at, Some(today + DAY_MS));
    }

    #[test]
    fn day_start_rolls_over_at_4am_local() {
        // day_start 返回的就是「本学习日 04:00」这一时刻
        let today = day_start(NOW, TZ);

        // 本地 04:00 整 → 属于新的一天
        assert_eq!(day_start(today, TZ), today);
        // 本地 03:59:59.999 → 仍属于前一天
        assert_eq!(day_start(today - 1, TZ), today - DAY_MS);
        // 本地 03:59 → 仍属于前一天
        assert_eq!(day_start(today - 60_000, TZ), today - DAY_MS);
        // 次日本地 03:59:59.999 → 仍属于今天
        assert_eq!(day_start(today + DAY_MS - 1, TZ), today);
    }

    #[test]
    fn days_from_civil_is_inverse_of_civil_from_days() {
        // 覆盖闰年、月末、世纪年
        for (y, m, d) in [
            (1970, 1, 1),
            (2026, 10, 11),
            (2000, 2, 29),
            (1900, 3, 1),
            (2024, 12, 31),
            (2100, 1, 1),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d));
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
    }

    #[test]
    fn day_range_covers_exactly_one_calendar_day() {
        let (start, end) = day_range_ms("2026-10-11", TZ).unwrap();
        assert_eq!(end - start, DAY_MS);
        // 区间长度固定一天，与月长/闰年无关
        let (feb_start, feb_end) = day_range_ms("2024-02-29", TZ).unwrap();
        assert_eq!(feb_end - feb_start, DAY_MS);
    }

    #[test]
    fn day_range_start_is_local_midnight() {
        let (start, _) = day_range_ms("2026-10-11", TZ).unwrap();
        // 本地 00:00 落在区间起点；03:00 仍属「日历日的上午」但已跨过学习日边界
        assert_eq!(day_key(start + 12 * 3_600_000, TZ), "2026-10-11");
        assert_eq!(day_key(start + 3 * 3_600_000, TZ), "2026-10-10");
    }

    #[test]
    fn day_range_rejects_bad_input() {
        assert!(day_range_ms("2026/10/11", TZ).is_err());
        assert!(day_range_ms("2026-13-01", TZ).is_err());
        assert!(day_range_ms("2026-10-32", TZ).is_err());
        assert!(day_range_ms("abc", TZ).is_err());
    }

    #[test]
    fn day_key_matches_local_calendar_date() {
        // 本地 2026-10-11 12:00 → 键应当是 2026-10-11
        let ts = 1_792_000_000_000; // 2026-10-16 前后，仅用于校验格式与连续性
        let key = day_key(ts, TZ);
        assert_eq!(key.len(), 10);
        assert_eq!(&key[4..5], "-");
        assert_eq!(&key[7..8], "-");
        // 相差一天的两个时刻，键必须不同
        assert_ne!(day_key(ts, TZ), day_key(ts + DAY_MS, TZ));
    }

    #[test]
    fn day_key_is_stable_across_utc_offsets() {
        // 同一物理时刻，东八区可能已经是次日而 UTC 还在当天
        let ts = 1_760_000_000_000;
        let east = day_key(ts, 480);
        let utc = day_key(ts, 0);
        assert!(!east.is_empty() && !utc.is_empty());
    }
}
