//! 手机端 SQLite 数据层。
//!
//! 最重要的一条不变式：**同步进来的记录只能刷新词义字段，
//! 绝不能覆盖 `state/ease/interval/reps/lapses/due_at/first_seen/last_review`**。
//! 因为 PC 端不知道学习进度，而重复收藏在 PC 端是常态（A4 无反向同步）。

use crate::sm2::{self, CardState, Grade};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS words (
    seq          INTEGER PRIMARY KEY,
    id           TEXT    NOT NULL UNIQUE,
    word         TEXT    NOT NULL,
    normalized   TEXT    NOT NULL,
    phonetic_us  TEXT,
    phonetic_uk  TEXT,
    audio_us     TEXT,
    audio_uk     TEXT,
    dit          TEXT,
    usual_dict   TEXT,
    exchange     TEXT,
    added_at     INTEGER,
    state        INTEGER NOT NULL DEFAULT 0,
    ease         REAL    NOT NULL DEFAULT 2.5,
    interval     INTEGER NOT NULL DEFAULT 0,
    reps         INTEGER NOT NULL DEFAULT 0,
    lapses       INTEGER NOT NULL DEFAULT 0,
    due_at       INTEGER,
    first_seen   INTEGER,
    last_review  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_words_due  ON words(state, due_at);
CREATE INDEX IF NOT EXISTS idx_words_norm ON words(normalized);

CREATE TABLE IF NOT EXISTS sentences (
    seq          INTEGER PRIMARY KEY,
    id           TEXT    NOT NULL UNIQUE,
    text         TEXT    NOT NULL,
    trans        TEXT,
    added_at     INTEGER,
    state        INTEGER NOT NULL DEFAULT 0,
    ease         REAL    NOT NULL DEFAULT 2.5,
    interval     INTEGER NOT NULL DEFAULT 0,
    reps         INTEGER NOT NULL DEFAULT 0,
    lapses       INTEGER NOT NULL DEFAULT 0,
    due_at       INTEGER,
    first_seen   INTEGER,
    last_review  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_sent_due ON sentences(state, due_at);

CREATE TABLE IF NOT EXISTS daily_stats (
    day         TEXT PRIMARY KEY,
    new_done    INTEGER NOT NULL DEFAULT 0,
    review_done INTEGER NOT NULL DEFAULT 0,
    study_ms    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sync_state (
    k TEXT PRIMARY KEY,
    v TEXT
);
"#;

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("打开本地库失败: {e}"))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("初始化本地库失败: {e}"))?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(conn)
}

fn json_column(value: Option<&Value>, fallback: Value) -> String {
    value
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or(fallback)
        .to_string()
}

fn text_of(record: &Value, key: &str) -> Option<String> {
    record
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn int_of(record: &Value, key: &str) -> Option<i64> {
    record.get(key).and_then(|v| v.as_i64())
}

/// 落库一条 PC 端下发的单词记录。
///
/// `ON CONFLICT` 分支**故意不含学习状态列**——这是「重复收藏不重置进度」的实现点。
pub fn apply_word_record(conn: &Connection, record: &Value) -> Result<(), String> {
    let id = text_of(record, "id").ok_or("记录缺少 id")?;
    let word = text_of(record, "word").ok_or("记录缺少 word")?;
    let normalized = text_of(record, "normalized").unwrap_or_else(|| word.to_lowercase());
    let seq = int_of(record, "seq").ok_or("记录缺少 seq")?;

    conn.execute(
        "INSERT INTO words (seq, id, word, normalized, phonetic_us, phonetic_uk, audio_us, \
         audio_uk, dit, usual_dict, exchange, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT(id) DO UPDATE SET \
            seq = excluded.seq, word = excluded.word, normalized = excluded.normalized, \
            phonetic_us = excluded.phonetic_us, phonetic_uk = excluded.phonetic_uk, \
            audio_us = excluded.audio_us, audio_uk = excluded.audio_uk, dit = excluded.dit, \
            usual_dict = excluded.usual_dict, exchange = excluded.exchange",
        params![
            seq,
            id,
            word,
            normalized,
            text_of(record, "phonetic_us"),
            text_of(record, "phonetic_uk"),
            text_of(record, "audio_us"),
            text_of(record, "audio_uk"),
            text_of(record, "dit"),
            json_column(record.get("usual_dict"), json!([])),
            json_column(record.get("exchange"), json!({})),
            int_of(record, "added_at").unwrap_or(0),
        ],
    )
    .map_err(|e| format!("写入单词失败: {e}"))?;

    Ok(())
}

pub fn apply_sentence_record(conn: &Connection, record: &Value) -> Result<(), String> {
    let id = text_of(record, "id").ok_or("记录缺少 id")?;
    let text = text_of(record, "text").ok_or("记录缺少 text")?;
    let seq = int_of(record, "seq").ok_or("记录缺少 seq")?;

    conn.execute(
        "INSERT INTO sentences (seq, id, text, trans, added_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(id) DO UPDATE SET \
            seq = excluded.seq, text = excluded.text, trans = excluded.trans",
        params![
            seq,
            id,
            text,
            text_of(record, "trans"),
            int_of(record, "added_at").unwrap_or(0),
        ],
    )
    .map_err(|e| format!("写入句子失败: {e}"))?;

    Ok(())
}

pub fn get_last_seq(conn: &Connection) -> i64 {
    conn.query_row("SELECT v FROM sync_state WHERE k = 'last_seq'", [], |row| {
        row.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
    .and_then(|v| v.parse::<i64>().ok())
    .unwrap_or(0)
}

pub fn set_last_seq(conn: &Connection, seq: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sync_state (k, v) VALUES ('last_seq', ?1) \
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![seq.to_string()],
    )
    .map_err(|e| format!("保存同步游标失败: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct WordRow {
    pub seq: i64,
    pub id: String,
    pub word: String,
    pub dit: Option<String>,
    pub usual_dict: Value,
    pub phonetic_us: Option<String>,
    pub phonetic_uk: Option<String>,
    pub audio_us: Option<String>,
    pub audio_uk: Option<String>,
    pub state: i32,
    pub ease: f64,
    pub interval: i64,
    pub reps: i32,
    pub lapses: i32,
    pub due_at: Option<i64>,
}

fn word_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WordRow> {
    let usual: Option<String> = row.get(4)?;
    Ok(WordRow {
        seq: row.get(0)?,
        id: row.get(1)?,
        word: row.get(2)?,
        dit: row.get(3)?,
        usual_dict: usual
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!([])),
        phonetic_us: row.get(5)?,
        phonetic_uk: row.get(6)?,
        audio_us: row.get(7)?,
        audio_uk: row.get(8)?,
        state: row.get(9)?,
        ease: row.get(10)?,
        interval: row.get(11)?,
        reps: row.get(12)?,
        lapses: row.get(13)?,
        due_at: row.get(14)?,
    })
}

const WORD_ROW_COLS: &str = "seq, id, word, dit, usual_dict, phonetic_us, phonetic_uk, \
                             audio_us, audio_uk, state, ease, interval, reps, lapses, due_at";

/// 取今日「到期复习」队列：`state IN (1,2)` 且 `due_at <= now`。
pub fn due_words(conn: &Connection, now_ms: i64, limit: i64) -> Result<Vec<WordRow>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {WORD_ROW_COLS} FROM words \
             WHERE state IN (1, 2) AND due_at IS NOT NULL AND due_at <= ?1 \
             ORDER BY due_at ASC LIMIT ?2"
        ))
        .map_err(|e| format!("准备到期查询失败: {e}"))?;
    let rows = stmt
        .query_map(params![now_ms, limit], word_from_row)
        .map_err(|e| format!("查询到期单词失败: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取到期单词失败: {e}"))
}

/// 取今日「新词」队列（`state = 0`），按 seq 升序。
pub fn new_words(conn: &Connection, limit: i64) -> Result<Vec<WordRow>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {WORD_ROW_COLS} FROM words WHERE state = 0 ORDER BY seq ASC LIMIT ?1"
        ))
        .map_err(|e| format!("准备新词查询失败: {e}"))?;
    let rows = stmt
        .query_map(params![limit], word_from_row)
        .map_err(|e| format!("查询新词失败: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取新词失败: {e}"))
}

const CARD_COLS: &str = "state, ease, interval, reps, lapses, due_at, first_seen, last_review";

/// `table` 走内部白名单，不接受外部输入。
fn card_table(table: &str) -> Result<&str, String> {
    match table {
        "words" | "sentences" => Ok(table),
        other => Err(format!("未知的表: {other}")),
    }
}

pub fn card_state_of(
    conn: &Connection,
    table: &str,
    id: &str,
) -> Result<Option<CardState>, String> {
    let table = card_table(table)?;
    conn.query_row(
        &format!("SELECT {CARD_COLS} FROM {table} WHERE id = ?1"),
        params![id],
        |row| {
            Ok(CardState {
                state: row.get(0)?,
                ease: row.get(1)?,
                interval: row.get(2)?,
                reps: row.get(3)?,
                lapses: row.get(4)?,
                due_at: row.get(5)?,
                first_seen: row.get(6)?,
                last_review: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取学习状态失败: {e}"))
}

pub fn word_state(conn: &Connection, id: &str) -> Result<Option<CardState>, String> {
    card_state_of(conn, "words", id)
}

/// 记录一次评分：读状态 → SM-2 → 写回。`table` 为 `words` 或 `sentences`。
pub fn grade_card(
    conn: &Connection,
    table: &str,
    id: &str,
    grade: Grade,
    now_ms: i64,
    utc_offset_minutes: i64,
) -> Result<CardState, String> {
    let table = card_table(table)?;
    let prev = card_state_of(conn, table, id)?.unwrap_or_default();
    let next = sm2::apply(&prev, grade, now_ms, utc_offset_minutes);

    let changed = conn
        .execute(
            &format!(
                "UPDATE {table} SET state = ?1, ease = ?2, interval = ?3, reps = ?4, lapses = ?5, \
                 due_at = ?6, first_seen = ?7, last_review = ?8 WHERE id = ?9"
            ),
            params![
                next.state,
                next.ease,
                next.interval,
                next.reps,
                next.lapses,
                next.due_at,
                next.first_seen,
                next.last_review,
                id
            ],
        )
        .map_err(|e| format!("保存学习状态失败: {e}"))?;

    if changed == 0 {
        return Err(format!("记录不存在: {id}"));
    }
    Ok(next)
}

pub fn grade_word(
    conn: &Connection,
    id: &str,
    grade: Grade,
    now_ms: i64,
    utc_offset_minutes: i64,
) -> Result<CardState, String> {
    grade_card(conn, "words", id, grade, now_ms, utc_offset_minutes)
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub word_count: i64,
    pub sentence_count: i64,
    pub new_count: i64,
    pub due_count: i64,
}

pub fn stats(conn: &Connection, now_ms: i64) -> Result<Stats, String> {
    let total = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, [], |row| row.get(0))
            .map_err(|e| format!("统计失败: {e}"))
    };
    let with_now = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, params![now_ms], |row| row.get(0))
            .map_err(|e| format!("统计失败: {e}"))
    };
    Ok(Stats {
        word_count: total("SELECT COUNT(*) FROM words")?,
        sentence_count: total("SELECT COUNT(*) FROM sentences")?,
        new_count: total("SELECT COUNT(*) FROM words WHERE state = 0")?,
        due_count: with_now(
            "SELECT COUNT(*) FROM words \
             WHERE state IN (1, 2) AND due_at IS NOT NULL AND due_at <= ?1",
        )?,
    })
}

/// 累加今日统计（学习时长）。
pub fn add_study_ms(conn: &Connection, day: &str, delta_ms: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO daily_stats (day, study_ms) VALUES (?1, ?2) \
         ON CONFLICT(day) DO UPDATE SET study_ms = study_ms + excluded.study_ms",
        params![day, delta_ms],
    )
    .map_err(|e| format!("累计学习时长失败: {e}"))?;
    Ok(())
}

pub fn get_study_ms(conn: &Connection, day: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT study_ms FROM daily_stats WHERE day = ?1",
        params![day],
        |row| row.get(0),
    )
    .optional()
    .map(|v| v.unwrap_or(0))
    .map_err(|e| format!("读取学习时长失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: i64 = 480;
    const NOW: i64 = 1_760_000_000_000;

    fn word_record() -> Value {
        json!({
            "seq": 1,
            "id": "abc123",
            "word": "scout",
            "normalized": "scout",
            "phonetic_us": "/skaʊt/",
            "phonetic_uk": "/skaʊt/",
            "audio_us": "https://dict.youdao.com/dictvoice?type=0&audio=scout",
            "audio_uk": "https://dict.youdao.com/dictvoice?type=1&audio=scout",
            "dit": "n. 侦察员",
            "usual_dict": [{"pos": "n.", "values": ["侦察员"]}],
            "exchange": {"word_pl": ["scouts"]},
            "added_at": NOW,
        })
    }

    #[test]
    fn sync_record_inserts_new_card_as_unlearned() {
        let conn = open_in_memory().unwrap();
        apply_word_record(&conn, &word_record()).unwrap();

        let card = word_state(&conn, "abc123").unwrap().unwrap();
        assert_eq!(card.state, 0);
        assert_eq!(card.reps, 0);
        assert_eq!(card.ease, 2.5);
        let words = new_words(&conn, 10).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].word, "scout");
        assert_eq!(words[0].usual_dict[0]["pos"], "n.");
    }

    /// 目标成功标准 4 的后半句：重复收藏只更新词义，**不重置学习进度**。
    #[test]
    fn resync_updates_gloss_but_keeps_learning_progress() {
        let conn = open_in_memory().unwrap();
        apply_word_record(&conn, &word_record()).unwrap();

        // 学两轮
        grade_word(&conn, "abc123", Grade::Good, NOW, TZ).unwrap();
        grade_word(&conn, "abc123", Grade::Good, NOW + sm2::DAY_MS, TZ).unwrap();
        let learned = word_state(&conn, "abc123").unwrap().unwrap();
        assert_eq!(learned.reps, 2);
        assert_eq!(learned.interval, 6);

        // PC 端再次收藏同一个词（词义有更新）
        let mut updated = word_record();
        updated["dit"] = json!("n. 侦察员；童子军");
        apply_word_record(&conn, &updated).unwrap();

        let after = word_state(&conn, "abc123").unwrap().unwrap();
        assert_eq!(after.reps, learned.reps, "reps 不能被重置");
        assert_eq!(after.interval, learned.interval, "interval 不能被重置");
        assert_eq!(after.due_at, learned.due_at, "due_at 不能被重置");
        assert!((after.ease - learned.ease).abs() < 1e-9, "ease 不能被重置");

        let dit: String = conn
            .query_row("SELECT dit FROM words WHERE id = 'abc123'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dit, "n. 侦察员；童子军", "词义必须被刷新");
    }

    #[test]
    fn resync_does_not_duplicate_rows() {
        let conn = open_in_memory().unwrap();
        for _ in 0..5 {
            apply_word_record(&conn, &word_record()).unwrap();
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM words", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn due_queue_only_contains_matured_cards() {
        let conn = open_in_memory().unwrap();
        apply_word_record(&conn, &word_record()).unwrap();

        // 新词不进到期队列
        assert!(due_words(&conn, NOW, 10).unwrap().is_empty());

        grade_word(&conn, "abc123", Grade::Again, NOW, TZ).unwrap();
        // 「不认识」→ 立刻到期
        assert_eq!(due_words(&conn, NOW, 10).unwrap().len(), 1);

        grade_word(&conn, "abc123", Grade::Good, NOW, TZ).unwrap();
        // 「我认识」→ 明天才到期
        assert!(due_words(&conn, NOW, 10).unwrap().is_empty());
        assert_eq!(due_words(&conn, NOW + sm2::DAY_MS, 10).unwrap().len(), 1);
    }

    #[test]
    fn sync_cursor_roundtrips() {
        let conn = open_in_memory().unwrap();
        assert_eq!(get_last_seq(&conn), 0);
        set_last_seq(&conn, 42).unwrap();
        assert_eq!(get_last_seq(&conn), 42);
        set_last_seq(&conn, 43).unwrap();
        assert_eq!(get_last_seq(&conn), 43);
    }

    #[test]
    fn sentences_use_separate_table_and_keep_progress() {
        let conn = open_in_memory().unwrap();
        let rec = json!({"seq": 2, "id": "s1", "text": "one two three", "trans": "一二三", "added_at": NOW});
        apply_sentence_record(&conn, &rec).unwrap();
        apply_sentence_record(&conn, &json!({"seq": 2, "id": "s1", "text": "one two three", "trans": "一二三（改）", "added_at": NOW})).unwrap();

        let (count, trans): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), MAX(trans) FROM sentences",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(trans, "一二三（改）");

        let st = stats(&conn, NOW).unwrap();
        assert_eq!(st.word_count, 0);
        assert_eq!(st.sentence_count, 1);
    }

    #[test]
    fn study_time_accumulates_per_day() {
        let conn = open_in_memory().unwrap();
        add_study_ms(&conn, "2026-10-11", 60_000).unwrap();
        add_study_ms(&conn, "2026-10-11", 30_000).unwrap();
        assert_eq!(get_study_ms(&conn, "2026-10-11").unwrap(), 90_000);
        assert_eq!(get_study_ms(&conn, "2026-10-12").unwrap(), 0, "跨日必须归零");
    }

    #[test]
    fn stats_reports_new_and_due_counts() {
        let conn = open_in_memory().unwrap();
        apply_word_record(&conn, &word_record()).unwrap();
        apply_word_record(
            &conn,
            &json!({"seq": 2, "id": "def456", "word": "harbor", "normalized": "harbor", "dit": "港口"}),
        )
        .unwrap();

        let st = stats(&conn, NOW).unwrap();
        assert_eq!(st.new_count, 2);
        assert_eq!(st.due_count, 0);

        grade_word(&conn, "abc123", Grade::Again, NOW, TZ).unwrap();
        let st = stats(&conn, NOW).unwrap();
        assert_eq!(st.new_count, 1);
        assert_eq!(st.due_count, 1);
    }
}
