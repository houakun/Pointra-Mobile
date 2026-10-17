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

-- 阅读文章：本地功能，与同步无关（PC 端不参与）。
-- body 存原始正文，段落切分在读取时做（见 articles::split_paragraphs），
-- 这样以后调整切分规则不需要迁移已有数据。
CREATE TABLE IF NOT EXISTS articles (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    keywords   TEXT NOT NULL DEFAULT '[]',
    origin     TEXT,
    source     TEXT NOT NULL DEFAULT 'import',
    created_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_articles_created ON articles(created_at DESC);

-- 查词 / 段落翻译缓存：k = 'w:<word>' 或 'p:<sha1>'。
-- 断网时靠它继续看已经查过的词与已翻译的段落（成功标准 6）。
CREATE TABLE IF NOT EXISTS article_cache (
    k          TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT 0
);
"#;

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("打开本地库失败: {e}"))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("初始化本地库失败: {e}"))?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    migrate(&conn)?;
    Ok(conn)
}

/// 老库补列：`CREATE TABLE IF NOT EXISTS` 不会给**已存在**的表加列，
/// 所以新增字段必须在这里显式补，否则老用户一升级就报「no such column」。
fn migrate(conn: &Connection) -> Result<(), String> {
    ensure_column(conn, "articles", "origin", "origin TEXT")
}

fn ensure_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> Result<(), String> {
    // 先 `collect` 成 Vec：迭代器借 `stmt`，且块尾表达式的临时值活到块结束，
    // 直接返回布尔会报 `stmt does not live long enough`。
    let columns: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| format!("读取表结构失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("读取表结构失败: {e}"))?;
        rows.filter_map(Result::ok).collect()
    };

    if !columns.iter().any(|name| name == column) {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {ddl}"), [])
            .map_err(|e| format!("补列 {table}.{column} 失败: {e}"))?;
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// 阅读文章（本地功能，与同步无关）
// ---------------------------------------------------------------------------

/// 导入文章的长度上限：防止一次粘贴把库撑爆（正文只是纯文本，10 万字符约 200KB）。
pub const MAX_ARTICLE_CHARS: usize = 100_000;

/// 从素材 id 解析「考试类型 + 年份」，供阅读页按 4/6 级与年份分类。
///
/// - `cet4-2015-06-1`、`cet4-2024-06-2-long-term-goals` → (`cet4`, Some(2015/2024))
/// - `builtin-*`（自写示例）、`import-*`（自己黏贴导入）→ (`""`, None)
///
/// 用 id 而不是 `origin` 解析：id 是稳定结构（由命名约定保证），
/// 而 origin 是给人看的中文句子，将来改文案就会把分类搞坏。
pub fn parse_exam_year(id: &str) -> (String, Option<i64>) {
    let mut parts = id.split('-');
    let exam = parts.next().unwrap_or_default();
    if exam != "cet4" && exam != "cet6" {
        return (String::new(), None);
    }
    let year = parts.next().and_then(|y| y.parse::<i64>().ok());
    (exam.to_string(), year)
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleSummary {
    pub id: String,
    pub title: String,
    /// `builtin` = 内置素材；`import` = 自己粘贴导入
    pub source: String,
    pub keywords: Vec<String>,
    /// 素材出处（真题选文写「某年某套真题 · 选自某刊」；导入文章为空）
    pub origin: String,
    /// 考试类型：`cet4` / `cet6`；自写示例与导入文章为空串
    pub exam: String,
    /// 真题年份（如 2015）；自写示例与导入文章为 `None`
    pub year: Option<i64>,
    pub paragraph_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleDetail {
    pub id: String,
    pub title: String,
    pub source: String,
    pub keywords: Vec<String>,
    pub origin: String,
    pub created_at: i64,
    pub paragraphs: Vec<String>,
}

fn json_array(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// `sync_state` 里的通用开关（已用它存 `last_seq`，这里复用同一张 k/v 表）。
pub fn get_sync_flag(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT v FROM sync_state WHERE k = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn set_sync_flag(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sync_state (k, v) VALUES (?1, ?2) \
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![key, value],
    )
    .map_err(|e| format!("保存状态标记失败: {e}"))?;
    Ok(())
}

/// 把内置素材**按 id 增量**同步进 `articles`：新增的插入，已存在的**刷新内容**。
///
/// 三个语义都是必需的：
/// - **增量**：以后新增文章，已装 App 的用户下次启动就能看到（这正是「素材太少」的成因）；
/// - **刷新**：素材被修正（改错字、补出处）也要能推送到已装的库
///   （用 `INSERT OR IGNORE` 会直接跳过，修正就永远生效不了）；
/// - **删除记账**（`articles_removed`）：用户手动删掉的内置文章不会自己复活。
///
/// 刷新时**不动 `created_at`**，否则每次启动列表顺序都会跳。
pub fn seed_builtin_articles(conn: &Connection) -> Result<usize, String> {
    let removed: Vec<String> = get_sync_flag(conn, "articles_removed")
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    let now = crate::settings::now_ms();
    let mut inserted = 0usize;
    for article in crate::articles::builtin_articles() {
        if removed.iter().any(|id| id == &article.id) {
            continue;
        }
        let keywords = serde_json::to_string(&article.keywords)
            .map_err(|e| format!("序列化核心词失败: {e}"))?;

        let existed = conn
            .query_row(
                "SELECT 1 FROM articles WHERE id = ?1",
                params![article.id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| format!("查询内置文章失败: {e}"))?
            .is_some();

        if existed {
            conn.execute(
                "UPDATE articles SET title = ?1, body = ?2, keywords = ?3, origin = ?4 \
                 WHERE id = ?5",
                params![
                    article.title,
                    article.body,
                    keywords,
                    article.origin,
                    article.id
                ],
            )
            .map_err(|e| format!("刷新内置文章失败: {e}"))?;
        } else {
            conn.execute(
                "INSERT INTO articles (id, title, body, keywords, origin, source, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'builtin', ?6)",
                params![
                    article.id,
                    article.title,
                    article.body,
                    keywords,
                    article.origin,
                    now
                ],
            )
            .map_err(|e| format!("写入内置文章失败: {e}"))?;
            inserted += 1;
        }
    }
    Ok(inserted)
}

pub fn list_articles(conn: &Connection) -> Result<Vec<ArticleSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, source, keywords, origin, body, created_at FROM articles \
             ORDER BY created_at DESC, id DESC",
        )
        .map_err(|e| format!("准备文章列表查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let body: String = row.get(5)?;
            let origin: Option<String> = row.get(4)?;
            let id: String = row.get(0)?;
            let (exam, year) = parse_exam_year(&id);
            Ok(ArticleSummary {
                id,
                title: row.get(1)?,
                source: row.get(2)?,
                keywords: json_array(row.get(3)?),
                origin: origin.unwrap_or_default(),
                exam,
                year,
                paragraph_count: crate::articles::split_paragraphs(&body).len() as i64,
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("查询文章列表失败: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("读取文章列表失败: {e}"))
}

pub fn get_article(conn: &Connection, id: &str) -> Result<Option<ArticleDetail>, String> {
    conn.query_row(
        "SELECT id, title, source, keywords, origin, body, created_at FROM articles WHERE id = ?1",
        params![id],
        |row| {
            let body: String = row.get(5)?;
            let origin: Option<String> = row.get(4)?;
            Ok(ArticleDetail {
                id: row.get(0)?,
                title: row.get(1)?,
                source: row.get(2)?,
                keywords: json_array(row.get(3)?),
                origin: origin.unwrap_or_default(),
                created_at: row.get(6)?,
                paragraphs: crate::articles::split_paragraphs(&body),
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取文章失败: {e}"))
}

/// 保存一篇导入的文章，返回落库后的详情（含切好的段落）。
///
/// id 由本机生成（`import-<毫秒>`）：文章是纯本地数据，不参与同步，
/// 所以不需要 PC 端下发的主键。
pub fn save_article(conn: &Connection, title: &str, body: &str) -> Result<ArticleDetail, String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("文章内容不能为空".to_string());
    }
    if body.chars().count() > MAX_ARTICLE_CHARS {
        return Err(format!(
            "文章太长了（{} 字，上限 {MAX_ARTICLE_CHARS} 字），请拆成几篇",
            body.chars().count()
        ));
    }
    let paragraphs = crate::articles::split_paragraphs(body);
    if paragraphs.is_empty() {
        return Err("文章内容不能为空".to_string());
    }
    let source_title = title.trim();
    let title = if source_title.is_empty() {
        "未命名文章"
    } else {
        source_title
    };

    let now = crate::settings::now_ms();
    let mut id = format!("import-{now}");
    let mut suffix = 1;
    while get_article(conn, &id)?.is_some() {
        suffix += 1;
        id = format!("import-{now}-{suffix}");
    }

    conn.execute(
        "INSERT INTO articles (id, title, body, keywords, origin, source, created_at) \
         VALUES (?1, ?2, ?3, '[]', NULL, 'import', ?4)",
        params![id, title, body, now],
    )
    .map_err(|e| format!("保存文章失败: {e}"))?;

    Ok(ArticleDetail {
        id,
        title: title.to_string(),
        source: "import".to_string(),
        keywords: Vec::new(),
        origin: String::new(),
        created_at: now,
        paragraphs,
    })
}

/// 删除单篇文章（前端负责二次确认）。
///
/// 删的是**内置素材**时会记账到 `articles_removed`：
/// 否则下次启动的增量 seed 会把它又插回来（用户会以为删除没生效）。
pub fn delete_article(conn: &Connection, id: &str) -> Result<(), String> {
    let changed = conn
        .execute("DELETE FROM articles WHERE id = ?1", params![id])
        .map_err(|e| format!("删除文章失败: {e}"))?;
    if changed == 0 {
        return Err(format!("文章不存在: {id}"));
    }

    if crate::articles::is_builtin_article(id) {
        let mut removed: Vec<String> = get_sync_flag(conn, "articles_removed")
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if !removed.iter().any(|r| r == id) {
            removed.push(id.to_string());
            let raw =
                serde_json::to_string(&removed).map_err(|e| format!("序列化删除记录失败: {e}"))?;
            set_sync_flag(conn, "articles_removed", &raw)?;
        }
    }
    Ok(())
}

// --- 查词 / 段落翻译缓存 -----------------------------------------------------

/// 缓存键：单词用 `w:<小写词>`，段落用 `p:<段落 sha1>`。
pub fn article_cache_get(conn: &Connection, key: &str) -> Option<Value> {
    conn.query_row(
        "SELECT payload FROM article_cache WHERE k = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
}

pub fn article_cache_put(
    conn: &Connection,
    key: &str,
    kind: &str,
    payload: &Value,
) -> Result<(), String> {
    let raw = serde_json::to_string(payload).map_err(|e| format!("序列化缓存失败: {e}"))?;
    conn.execute(
        "INSERT INTO article_cache (k, kind, payload, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(k) DO UPDATE SET kind = excluded.kind, payload = excluded.payload, \
         created_at = excluded.created_at",
        params![key, kind, raw, crate::settings::now_ms()],
    )
    .map_err(|e| format!("写入缓存失败: {e}"))?;
    Ok(())
}

// --- 本机收藏（阅读页「加入单词本」）----------------------------------------

/// 本机收藏的 id：`mobile-` 前缀 + 词形哈希。
///
/// **这是全项目唯一一处「非 PC 下发写入 `words` 表」的地方**，所以做了两道隔离：
/// 1. `id` 带 `mobile-` 前缀 → 永不与 PC 生成的 sha1 id 相撞；
/// 2. `seq` 走负值区段（见下）→ 永不与 PC 的正数 seq 撞主键，也动不到同步游标 `last_seq`。
///
/// 已知取舍：PC 端之后若也收藏同一个词，会多出一条记录（两套 id 空间互不覆盖），
/// 用户会在队列里各背一次。这是刻意选的：按 word 去重会破坏
/// 「重复收藏只刷新词义、不重置学习进度」的既有语义。
pub fn local_word_id(word: &str) -> String {
    format!(
        "mobile-{:016x}",
        crate::dict::fnv1a_64(&word.trim().to_lowercase())
    )
}

/// 本机收藏专用的负 seq：PC 的 seq 全是正整数，两者永远不会撞主键。
fn local_word_seq(id: &str) -> i64 {
    let hash = crate::dict::fnv1a_64(id) >> 1; // 最高位清零，下面取负不会溢出
    -((hash as i64) + 1) // 恒 <= -1
}

/// 这个词是否已在本机单词本里（用阅读页收藏同一套 id 规则）。
///
/// 词卡靠它决定星标显示实心还是空心：只靠组件内部状态的话，
/// 关掉词卡再打开就又会变成空心星，看起来像「收藏没成功」。
pub fn has_local_word(conn: &Connection, word: &str) -> Result<bool, String> {
    let id = local_word_id(word);
    conn.query_row("SELECT 1 FROM words WHERE id = ?1", params![id], |_| Ok(()))
        .optional()
        .map(|found| found.is_some())
        .map_err(|e| format!("查询单词失败: {e}"))
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalWordAdd {
    pub id: String,
    pub word: String,
    /// `true` = 该词本机已经收藏过，这次只刷新了词义（进度未动）
    pub existed: bool,
}

/// 把阅读页查到的词写进本机词库，立即可背。
pub fn add_local_word(
    conn: &Connection,
    word: &str,
    entry: &crate::dict::WordLookup,
) -> Result<LocalWordAdd, String> {
    let word = word.trim();
    if word.is_empty() {
        return Err("单词为空".to_string());
    }
    let id = local_word_id(word);
    let existed = conn
        .query_row("SELECT 1 FROM words WHERE id = ?1", params![id], |_| Ok(()))
        .optional()
        .map_err(|e| format!("查询单词失败: {e}"))?
        .is_some();

    let usual_dict =
        serde_json::to_string(&entry.usual_dict).map_err(|e| format!("序列化释义失败: {e}"))?;
    let exchange =
        serde_json::to_string(&entry.exchange).map_err(|e| format!("序列化词形变化失败: {e}"))?;

    // ON CONFLICT 分支同样不含学习状态列：重复收藏不重置进度。
    conn.execute(
        "INSERT INTO words (seq, id, word, normalized, phonetic_us, phonetic_uk, dit, \
         usual_dict, exchange, added_at, state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0) \
         ON CONFLICT(id) DO UPDATE SET \
            word = excluded.word, normalized = excluded.normalized, \
            phonetic_us = excluded.phonetic_us, phonetic_uk = excluded.phonetic_uk, \
            dit = excluded.dit, usual_dict = excluded.usual_dict, exchange = excluded.exchange",
        params![
            local_word_seq(&id),
            id,
            word,
            word.to_lowercase(),
            entry.phonetic_us,
            entry.phonetic_uk,
            entry.dit,
            usual_dict,
            exchange,
            crate::settings::now_ms(),
        ],
    )
    .map_err(|e| format!("加入单词本失败: {e}"))?;

    Ok(LocalWordAdd {
        id,
        word: word.to_string(),
        existed,
    })
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
            .query_row("SELECT dit FROM words WHERE id = 'abc123'", [], |r| {
                r.get(0)
            })
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
            .query_row("SELECT COUNT(*), MAX(trans) FROM sentences", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
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
        assert_eq!(
            get_study_ms(&conn, "2026-10-12").unwrap(),
            0,
            "跨日必须归零"
        );
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

    // --- 阅读文章 ---------------------------------------------------------

    /// 升级场景：老用户已有的库再来一次 `SCHEMA` 必须平滑加表、不丢数据。
    #[test]
    fn existing_database_gains_article_tables_without_losing_data() {
        let conn = open_in_memory().unwrap();
        // 回退到「升级前」的结构：把新表拆掉，只留同步相关的数据。
        conn.execute_batch("DROP TABLE articles; DROP TABLE article_cache;")
            .unwrap();
        apply_word_record(&conn, &word_record()).unwrap();
        set_last_seq(&conn, 7).unwrap();

        // App 升级后重新 open 时会重跑 SCHEMA
        conn.execute_batch(SCHEMA).unwrap();

        let words: i64 = conn
            .query_row("SELECT COUNT(*) FROM words", [], |r| r.get(0))
            .unwrap();
        assert_eq!(words, 1, "升级不能丢已有单词");
        assert_eq!(get_last_seq(&conn), 7, "升级不能丢同步游标");
        let articles: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(articles, 0);
    }

    /// 素材是**增量**补插的（以后新增真题，已装 App 的用户也要能看到），
    /// 但用户手动删掉的内置文章不能自己复活。
    #[test]
    fn builtin_seed_is_incremental_and_respects_deletions() {
        let conn = open_in_memory().unwrap();
        let total = crate::articles::builtin_articles().len();

        assert_eq!(seed_builtin_articles(&conn).unwrap(), total);
        assert_eq!(seed_builtin_articles(&conn).unwrap(), 0, "已存在的不能再插");
        assert_eq!(list_articles(&conn).unwrap().len(), total);

        // 模拟「App 升级后多了一篇内置素材」：直接删除表里的记录（**不**记账），
        // 再 seed 应该补回来 —— 这就是老用户能拿到新文章的原因。
        conn.execute(
            "DELETE FROM articles WHERE id = 'cet4-2023-12-1-climate-food'",
            [],
        )
        .unwrap();
        assert_eq!(seed_builtin_articles(&conn).unwrap(), 1, "缺失的要补插");
        assert_eq!(list_articles(&conn).unwrap().len(), total);

        // 走正常删除路径（会记账）→ 再 seed 不能复活
        delete_article(&conn, "cet4-2023-12-1-climate-food").unwrap();
        assert_eq!(seed_builtin_articles(&conn).unwrap(), 0);
        assert_eq!(
            list_articles(&conn).unwrap().len(),
            total - 1,
            "用户删掉的内置文章不能自己复活"
        );
    }

    /// 素材被修正（改错字、补出处）时必须能推送到已装的库：
    /// 曾经的 `INSERT OR IGNORE` 会直接跳过已存在的 id，修正就永远生效不了。
    #[test]
    fn builtin_seed_refreshes_changed_article_content() {
        let conn = open_in_memory().unwrap();
        seed_builtin_articles(&conn).unwrap();

        let id = "cet4-2025-12-1-bike-lanes";
        conn.execute(
            "UPDATE articles SET title = '旧标题', body = '旧正文', origin = '旧出处' WHERE id = ?1",
            rusqlite::params![id],
        )
        .unwrap();

        // 已存在 → 不算新增，但内容要被刷新
        assert_eq!(seed_builtin_articles(&conn).unwrap(), 0, "已存在的不算新增");
        let (title, body, origin): (String, String, String) = conn
            .query_row(
                "SELECT title, body, origin FROM articles WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "The Battle Over Bike Lanes Needs a Mindset Shift");
        assert!(body.contains("Five years ago"), "正文也要被刷新");
        assert!(origin.contains("真题"), "出处也要被刷新（实际：{origin}）");
    }

    /// 老库（`articles` 表没有 `origin` 列）升级时必须自动补列，
    /// 否则老用户一打开就报 `no such column: origin`。
    #[test]
    fn legacy_articles_table_gains_origin_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (id TEXT PRIMARY KEY, title TEXT NOT NULL, body TEXT NOT NULL, \
             keywords TEXT NOT NULL DEFAULT '[]', source TEXT NOT NULL DEFAULT 'import', \
             created_at INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (id, title, body, keywords, source, created_at) \
             VALUES ('a1', '老文章', 'body', '[]', 'import', 1)",
            [],
        )
        .unwrap();

        // 升级：跑一次 SCHEMA（不改已存在的表）+ 迁移
        conn.execute_batch(SCHEMA).unwrap();
        migrate(&conn).unwrap();

        let origin: Option<String> = conn
            .query_row("SELECT origin FROM articles WHERE id = 'a1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(origin.is_none(), "补列后老数据为 NULL");

        let title: String = conn
            .query_row("SELECT title FROM articles WHERE id = 'a1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "老文章", "补列不能丢数据");

        // 迁移是幂等的：再来一次不能报错
        migrate(&conn).unwrap();
    }

    #[test]
    fn imported_article_roundtrip() {
        let conn = open_in_memory().unwrap();
        let saved = save_article(
            &conn,
            "  我的文章  ",
            "First line\nwrapped\n\nSecond paragraph.\n",
        )
        .unwrap();
        assert_eq!(saved.title, "我的文章", "标题要 trim");
        assert_eq!(saved.source, "import");
        assert_eq!(
            saved.paragraphs,
            vec!["First line wrapped", "Second paragraph."]
        );

        let list = list_articles(&conn).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].paragraph_count, 2);
        assert_eq!(list[0].id, saved.id);

        let got = get_article(&conn, &saved.id).unwrap().unwrap();
        assert_eq!(got.paragraphs, saved.paragraphs);
        assert_eq!(got.title, saved.title);

        delete_article(&conn, &saved.id).unwrap();
        assert!(get_article(&conn, &saved.id).unwrap().is_none());
        assert!(
            delete_article(&conn, &saved.id).is_err(),
            "删不存在的文章要报错"
        );
    }

    #[test]
    fn save_article_validates_input() {
        let conn = open_in_memory().unwrap();
        assert!(
            save_article(&conn, "t", "   \n\n ").is_err(),
            "空正文要拒绝"
        );
        let huge = "a".repeat(MAX_ARTICLE_CHARS + 1);
        assert!(save_article(&conn, "t", &huge).is_err(), "超长正文要拒绝");
        let saved = save_article(&conn, "   ", "Some text.").unwrap();
        assert_eq!(saved.title, "未命名文章");
    }

    #[test]
    fn parses_exam_and_year_from_article_id() {
        assert_eq!(
            parse_exam_year("cet4-2015-06-1"),
            ("cet4".to_string(), Some(2015))
        );
        assert_eq!(
            parse_exam_year("cet6-2024-06-2-long-term-goals"),
            ("cet6".to_string(), Some(2024))
        );
        // 自写示例与导入文章不归到任何考试/年份，前端会把它们放进「示例与导入」组
        assert_eq!(parse_exam_year("builtin-food-label"), (String::new(), None));
        assert_eq!(
            parse_exam_year("import-1760000000000"),
            (String::new(), None)
        );
    }

    #[test]
    fn article_cache_roundtrips() {
        let conn = open_in_memory().unwrap();
        assert!(article_cache_get(&conn, "w:one").is_none());
        article_cache_put(&conn, "w:one", "word", &json!({"dit": "一"})).unwrap();
        article_cache_put(&conn, "w:one", "word", &json!({"dit": "一个"})).unwrap();
        assert_eq!(article_cache_get(&conn, "w:one").unwrap()["dit"], "一个");
    }

    // --- 本机收藏（阅读页）------------------------------------------------

    fn lookup_of(word: &str) -> crate::dict::WordLookup {
        crate::dict::WordLookup {
            word: word.to_string(),
            phonetic_us: Some("sɪɡˈnɪfɪkənt".to_string()),
            phonetic_uk: None,
            dit: Some("adj. 重要的；显著的".to_string()),
            usual_dict: vec![crate::dict::DictGroup {
                pos: "adj.".to_string(),
                values: vec!["重要的".to_string()],
            }],
            exchange: json!({"word_pl": ["significants"]}),
            sentence: None,
            sentence_trans: None,
        }
    }

    /// 成功标准 5：收藏后立即是「新词」，能进今日队列。
    #[test]
    fn local_word_enters_the_new_queue() {
        let conn = open_in_memory().unwrap();
        let added = add_local_word(&conn, "Significant", &lookup_of("significant")).unwrap();

        assert!(added.id.starts_with("mobile-"), "id 要有前缀：{}", added.id);
        assert!(!added.existed);

        let seq: i64 = conn
            .query_row(
                "SELECT seq FROM words WHERE id = ?1",
                params![added.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(seq < 0, "本机收藏的 seq 必须落在负值区段，实际 {seq}");

        let queue = new_words(&conn, 10).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].word, "Significant");
        assert_eq!(queue[0].usual_dict[0]["pos"], "adj.");
        assert_eq!(stats(&conn, NOW).unwrap().new_count, 1);
    }

    #[test]
    fn local_word_resave_keeps_progress_and_does_not_duplicate() {
        let conn = open_in_memory().unwrap();
        let first = add_local_word(&conn, "significant", &lookup_of("significant")).unwrap();
        grade_word(&conn, &first.id, Grade::Good, NOW, TZ).unwrap();
        let learned = word_state(&conn, &first.id).unwrap().unwrap();

        let again = add_local_word(&conn, "significant", &lookup_of("significant")).unwrap();
        assert!(again.existed, "第二次收藏要能识别出已存在");
        assert_eq!(again.id, first.id);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM words", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "同一个词只能有一条本机记录");

        let after = word_state(&conn, &first.id).unwrap().unwrap();
        assert_eq!(after.reps, learned.reps, "重复收藏不能重置进度");
        assert_eq!(after.interval, learned.interval);
    }

    /// 隔离验证：本机收藏与 PC 后来下发的同一个词必须**共存而不互相污染**。
    #[test]
    fn local_word_coexists_with_pc_record_of_the_same_word() {
        let conn = open_in_memory().unwrap();
        let local = add_local_word(&conn, "significant", &lookup_of("significant")).unwrap();
        // 用户在本机背了几轮
        grade_word(&conn, &local.id, Grade::Good, NOW, TZ).unwrap();
        let local_state = word_state(&conn, &local.id).unwrap().unwrap();

        // PC 端随后也收藏了同一个词并下发（PC 自己生成 id/正 seq）
        apply_word_record(
            &conn,
            &json!({
                "seq": 501, "id": "pcsha1", "word": "significant", "normalized": "significant",
                "dit": "adj. 重要的（PC 版）", "usual_dict": [{"pos": "adj.", "values": ["重要的"]}],
                "exchange": {}, "added_at": NOW
            }),
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM words", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "两套 id 空间共存：本机一条 + PC 一条");

        // 本机那条的学习进度不能被 PC 的下发带走
        let after = word_state(&conn, &local.id).unwrap().unwrap();
        assert_eq!(after.reps, local_state.reps);
        assert_eq!(after.interval, local_state.interval);
        // PC 那条是全新的（没被本机进度污染）
        let pc = word_state(&conn, "pcsha1").unwrap().unwrap();
        assert_eq!(pc.reps, 0);
        assert_eq!(pc.state, 0);

        // 同步游标不受负 seq 影响
        assert_eq!(get_last_seq(&conn), 0);
    }
}
