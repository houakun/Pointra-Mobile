//! 学习队列与每日目标（P4 核心逻辑，纯 Rust + SQLite，不依赖 Tauri）。
//!
//! 队列规则（需求文档 §6.3）：
//! `本次队列 = 到期复习（按 due_at 升序）++ 新卡（按 seq 升序）`，
//! 单词与句子**共用同一个队列**，各自受每日上限约束。

use crate::db;
use crate::sm2::{self, Grade};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const DEFAULT_NEW_PER_DAY: i64 = 20;
pub const DEFAULT_REVIEW_PER_DAY: i64 = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DailyGoals {
    pub new_per_day: i64,
    pub review_per_day: i64,
}

impl Default for DailyGoals {
    fn default() -> Self {
        Self {
            new_per_day: DEFAULT_NEW_PER_DAY,
            review_per_day: DEFAULT_REVIEW_PER_DAY,
        }
    }
}

/// `(kind, table, 正面文本列)`。表名走内部常量，不接受外部输入。
const TABLES: [(&str, &str, &str); 2] =
    [("word", "words", "word"), ("sentence", "sentences", "text")];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueEntry {
    pub kind: String,
    pub id: String,
    pub seq: i64,
    /// 卡片正面文本：单词卡 = 单词，句子卡 = 英文原句
    pub front: String,
    /// 进队时的学习状态（0=新卡；1/2=到期复习）。
    /// 前端评分时靠它判断该计「今日新词」还是「今日复习」，
    /// 免得为了拿状态再多跑一次 `get_card`。
    pub state: i32,
}

pub fn build_queue(
    conn: &Connection,
    now_ms: i64,
    goals: DailyGoals,
) -> Result<Vec<QueueEntry>, String> {
    let mut due: Vec<(i64, QueueEntry)> = Vec::new();
    let mut fresh: Vec<QueueEntry> = Vec::new();

    for (kind, table, front_col) in TABLES {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, seq, {front_col}, due_at, state FROM {table} \
                 WHERE state IN (1, 2) AND due_at IS NOT NULL AND due_at <= ?1 \
                 ORDER BY due_at ASC"
            ))
            .map_err(|e| format!("准备到期查询失败: {e}"))?;
        let rows = stmt
            .query_map(params![now_ms], |row| {
                Ok((
                    row.get::<_, i64>(3)?,
                    QueueEntry {
                        kind: kind.to_string(),
                        id: row.get(0)?,
                        seq: row.get(1)?,
                        front: row.get(2)?,
                        state: row.get(4)?,
                    },
                ))
            })
            .map_err(|e| format!("查询到期卡片失败: {e}"))?;
        for row in rows {
            due.push(row.map_err(|e| format!("读取到期卡片失败: {e}"))?);
        }

        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, seq, {front_col}, state FROM {table} WHERE state = 0 ORDER BY seq ASC"
            ))
            .map_err(|e| format!("准备新卡查询失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(QueueEntry {
                    kind: kind.to_string(),
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    front: row.get(2)?,
                    state: row.get(3)?,
                })
            })
            .map_err(|e| format!("查询新卡失败: {e}"))?;
        for row in rows {
            fresh.push(row.map_err(|e| format!("读取新卡失败: {e}"))?);
        }
    }

    due.sort_by_key(|(due_at, _)| *due_at);
    due.truncate(goals.review_per_day.max(0) as usize);
    fresh.sort_by_key(|entry| entry.seq);
    fresh.truncate(goals.new_per_day.max(0) as usize);

    let mut items: Vec<QueueEntry> = due.into_iter().map(|(_, entry)| entry).collect();
    items.extend(fresh);
    Ok(items)
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub review_done: i64,
    /// 分母 = min(到期总数, 每日复习上限)，对应截图里的「350/450」
    pub review_total: i64,
    pub new_done: i64,
    /// 分母 = min(未学总数, 每日新词上限)
    pub new_total: i64,
    pub study_ms: i64,
}

fn count_due(conn: &Connection, now_ms: i64) -> Result<i64, String> {
    let mut total: i64 = 0;
    for (_, table, _) in TABLES {
        // 带 ?1 的语句必须真的传参；不带参数的语句必须传空数组，否则 rusqlite 直接报错
        total += conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} \
                     WHERE state IN (1, 2) AND due_at IS NOT NULL AND due_at <= ?1"
                ),
                params![now_ms],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("统计到期卡片失败: {e}"))?;
    }
    Ok(total)
}

fn count_new(conn: &Connection) -> Result<i64, String> {
    let mut total: i64 = 0;
    for (_, table, _) in TABLES {
        total += conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE state = 0"),
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("统计新卡失败: {e}"))?;
    }
    Ok(total)
}

pub fn today_progress(
    conn: &Connection,
    now_ms: i64,
    utc_offset_minutes: i64,
    goals: DailyGoals,
) -> Result<Progress, String> {
    let day = sm2::day_key(now_ms, utc_offset_minutes);

    let (new_done, review_done, study_ms) = conn
        .query_row(
            "SELECT new_done, review_done, study_ms FROM daily_stats WHERE day = ?1",
            params![day],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("读取今日统计失败: {e}"))?
        .unwrap_or((0, 0, 0));

    // 分母 = min(当日剩余池 + 当日已完成, 上限)。
    // 只取「剩余池」的话，学完之后分母会掉到 0，出现「今日复习 3/0 / 待复习 -3」
    // 这种反直觉显示；把已完成的加回池子，才是「今天一共要复习多少」。
    let review_total = (count_due(conn, now_ms)? + review_done).min(goals.review_per_day.max(0));
    let new_total = (count_new(conn)? + new_done).min(goals.new_per_day.max(0));

    Ok(Progress {
        review_done,
        review_total,
        new_done,
        new_total,
        study_ms,
    })
}

/// 记录一次评分：更新卡片状态 + 累加今日统计。
///
/// `was_new` 由调用方判定（该卡进入本次会话前是否 `state = 0`），
/// 决定这一笔计入「今日新词」还是「今日复习」。
pub fn record_grade(
    conn: &Connection,
    table: &str,
    id: &str,
    grade: Grade,
    now_ms: i64,
    utc_offset_minutes: i64,
    was_new: bool,
) -> Result<(), String> {
    db::grade_card(conn, table, id, grade, now_ms, utc_offset_minutes)?;

    let day = sm2::day_key(now_ms, utc_offset_minutes);
    let column = if was_new { "new_done" } else { "review_done" };
    conn.execute(
        &format!(
            "INSERT INTO daily_stats (day, {column}) VALUES (?1, 1) \
             ON CONFLICT(day) DO UPDATE SET {column} = {column} + 1"
        ),
        params![day],
    )
    .map_err(|e| format!("累加今日统计失败: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_sentence_record, apply_word_record};
    use serde_json::json;

    const TZ: i64 = 480;
    const NOW: i64 = 1_760_000_000_000;
    const DAY: i64 = sm2::DAY_MS;

    fn word(conn: &Connection, seq: i64, id: &str, text: &str) {
        apply_word_record(
            conn,
            &json!({"seq": seq, "id": id, "word": text, "normalized": text, "dit": "x"}),
        )
        .unwrap();
    }

    fn sentence(conn: &Connection, seq: i64, id: &str, text: &str) {
        apply_sentence_record(
            conn,
            &json!({"seq": seq, "id": id, "text": text, "trans": "x", "added_at": NOW}),
        )
        .unwrap();
    }

    #[test]
    fn queue_puts_due_reviews_before_new_cards() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");
        word(&conn, 2, "w2", "harbor");

        // w1 学过一次后「不认识」→ 立刻到期
        crate::db::grade_word(&conn, "w1", Grade::Again, NOW, TZ).unwrap();

        let queue = build_queue(&conn, NOW, DailyGoals::default()).unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].id, "w1", "到期复习必须排在新卡之前");
        assert_eq!(queue[1].id, "w2");
    }

    #[test]
    fn queue_mixes_words_and_sentences_by_due_time() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");
        sentence(&conn, 2, "s1", "one two three");

        crate::db::grade_word(&conn, "w1", Grade::Again, NOW, TZ).unwrap();
        db::grade_card(&conn, "sentences", "s1", Grade::Again, NOW, TZ).unwrap();

        let queue = build_queue(&conn, NOW, DailyGoals::default()).unwrap();
        assert_eq!(queue.len(), 2, "单词卡与句子卡共用同一个队列");
        assert!(queue.iter().any(|e| e.kind == "word"));
        assert!(queue.iter().any(|e| e.kind == "sentence"));
    }

    #[test]
    fn queue_respects_daily_caps() {
        let conn = db::open_in_memory().unwrap();
        for i in 1..=30 {
            word(&conn, i, &format!("w{i}"), &format!("word{i}"));
        }

        let goals = DailyGoals {
            new_per_day: 5,
            review_per_day: 100,
        };
        let queue = build_queue(&conn, NOW, goals).unwrap();
        assert_eq!(queue.len(), 5, "新词必须被每日上限截断");

        // 上限调大 → 立刻生效（无需重启）
        let bigger = DailyGoals {
            new_per_day: 12,
            ..goals
        };
        assert_eq!(build_queue(&conn, NOW, bigger).unwrap().len(), 12);
    }

    #[test]
    fn progress_denominators_use_min_of_pool_and_cap() {
        let conn = db::open_in_memory().unwrap();
        for i in 1..=30 {
            word(&conn, i, &format!("w{i}"), &format!("word{i}"));
        }
        let goals = DailyGoals {
            new_per_day: 20,
            review_per_day: 100,
        };

        let p = today_progress(&conn, NOW, TZ, goals).unwrap();
        assert_eq!(p.new_total, 20, "分母 = min(30 个未学, 上限 20)");
        assert_eq!(p.new_done, 0);
        assert_eq!(p.review_total, 0);

        let small = DailyGoals {
            new_per_day: 50,
            review_per_day: 100,
        };
        assert_eq!(
            today_progress(&conn, NOW, TZ, small).unwrap().new_total,
            30,
            "池子小于上限时，分母取池子大小"
        );
    }

    #[test]
    fn record_grade_counts_into_the_right_bucket() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");
        word(&conn, 2, "w2", "harbor");

        let goals = DailyGoals::default();
        record_grade(&conn, "words", "w1", Grade::Good, NOW, TZ, true).unwrap();
        record_grade(&conn, "words", "w2", Grade::Good, NOW, TZ, false).unwrap();

        let p = today_progress(&conn, NOW, TZ, goals).unwrap();
        assert_eq!(p.new_done, 1, "首次学习计入「今日新词」");
        assert_eq!(p.review_done, 1, "复习计入「今日复习」");
    }

    #[test]
    fn stats_reset_at_4am_boundary() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");
        record_grade(&conn, "words", "w1", Grade::Good, NOW, TZ, true).unwrap();
        assert_eq!(
            today_progress(&conn, NOW, TZ, DailyGoals::default())
                .unwrap()
                .new_done,
            1
        );

        // 次日 04:00 之后：新的学习日，计数从 0 开始
        let next_day = sm2::day_start(NOW, TZ) + DAY + 3_600_000;
        let p = today_progress(&conn, next_day, TZ, DailyGoals::default()).unwrap();
        assert_eq!(p.new_done, 0, "跨过 04:00 后今日统计必须归零");
        assert_eq!(p.study_ms, 0);
    }

    #[test]
    fn queue_excludes_graduated_cards() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");
        record_grade(&conn, "words", "w1", Grade::TooEasy, NOW, TZ, true).unwrap();

        assert!(build_queue(&conn, NOW, DailyGoals::default())
            .unwrap()
            .is_empty());
        assert!(
            build_queue(&conn, NOW + 400 * DAY, DailyGoals::default())
                .unwrap()
                .is_empty(),
            "「太简单」的词永不再入队"
        );
    }

    /// 队列条目要带上进队时的状态：新卡 0、到期复习 1/2。
    /// 前端靠它决定这一笔算「今日新词」还是「今日复习」。
    #[test]
    fn queue_entries_carry_state() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");
        word(&conn, 2, "w2", "harbor");

        // w1 先「我认识」再「不认识」→ 变成已到期的学习中（state=1）
        crate::db::grade_word(&conn, "w1", Grade::Good, NOW, TZ).unwrap();
        crate::db::grade_word(&conn, "w1", Grade::Again, NOW + DAY, TZ).unwrap();

        let queue = build_queue(&conn, NOW + DAY, DailyGoals::default()).unwrap();
        let w1 = queue.iter().find(|e| e.id == "w1").expect("w1 应在队列里");
        let w2 = queue.iter().find(|e| e.id == "w2").expect("w2 应在队列里");
        assert_eq!(w1.state, 1, "「不认识」后是学习中且已到期");
        assert_eq!(w2.state, 0, "没学过的仍是新卡");
    }

    #[test]
    fn again_card_reappears_in_the_same_session() {
        let conn = db::open_in_memory().unwrap();
        word(&conn, 1, "w1", "scout");

        record_grade(&conn, "words", "w1", Grade::Again, NOW, TZ, true).unwrap();
        let queue = build_queue(&conn, NOW, DailyGoals::default()).unwrap();
        assert_eq!(queue.len(), 1, "「不认识」的卡当日必须重现");
        assert_eq!(queue[0].id, "w1");
    }
}
