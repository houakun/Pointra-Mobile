//! 词库 / 句子本检索（P6 数据层）。
//!
//! 几万词规模下不能用 `LIKE '%kw%'`（全表扫描 + 无法走索引）。
//! 单词检索改用**前缀范围查询** `normalized >= lo AND normalized < hi`，
//! 它能命中 `idx_words_norm`（BINARY collation），由 `browse_words_uses_index` 测试锁死。
//!
//! 句子表没有 normalized 列，且句量远小于词量，因此句子搜索用包含匹配并接受全表扫描。

use crate::sm2;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    All,
    New,
    Learning,
    Review,
    Mastered,
}

impl StateFilter {
    fn sql(self) -> &'static str {
        match self {
            StateFilter::All => "1 = 1",
            StateFilter::New => "state = 0",
            StateFilter::Learning => "state = 1",
            StateFilter::Review => "state = 2",
            StateFilter::Mastered => "state = 3",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowseItem {
    pub id: String,
    pub text: String,
    /// 单词 = 中文释义；句子 = 中文译文
    pub gloss: Option<String>,
    pub state: i32,
    pub due_at: Option<i64>,
    /// 加入时间（毫秒），“按加入日期搜索”与列表展示都用它
    pub added_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowsePage {
    pub items: Vec<BrowseItem>,
    pub total: i64,
    pub has_more: bool,
}

/// 前缀 → `[lo, hi)` 半开区间。`hi` 用最大码位，保证覆盖 "abc" 及其后所有 "abc*"。
fn prefix_range(prefix: &str) -> (String, String) {
    let lo = prefix.trim().to_lowercase();
    let hi = format!("{lo}\u{10FFFF}");
    (lo, hi)
}

/// 词库检索条件。
///
/// 字段一多，位置参数就难读且容易调错顺序，所以换成结构体；
/// 新增筛选（比如「按加入日期」）只需要加字段。
#[derive(Debug, Clone, Copy)]
pub struct WordQuery<'a> {
    /// 单词前缀（空串 = 不筛）
    pub prefix: &'a str,
    /// 学习状态筛选
    pub state: StateFilter,
    /// 首字母筛选（与 `prefix` 互斥，`prefix` 优先）
    pub letter: Option<char>,
    /// 加入日期：**本地日历日** `YYYY-MM-DD`（`None` = 不筛）
    pub added_on: Option<&'a str>,
    /// 时区偏移（分钟），用于把 `added_on` 换成毫秒区间
    pub utc_offset_minutes: i64,
    /// `true` = 按加入时间倒序（新收藏的在前）；`false` = 按字母升序
    pub newest_first: bool,
}

impl Default for WordQuery<'_> {
    fn default() -> Self {
        Self {
            prefix: "",
            state: StateFilter::All,
            letter: None,
            added_on: None,
            utc_offset_minutes: 0,
            newest_first: false,
        }
    }
}

/// 分页收尾：多取一条判断 `has_more`，并把多余那条去掉。
fn finish(mut items: Vec<BrowseItem>, total: i64, limit: i64) -> BrowsePage {
    let has_more = items.len() as i64 > limit;
    if has_more {
        items.truncate(limit.max(0) as usize);
    }
    BrowsePage {
        items,
        total,
        has_more,
    }
}

/// 词库分页检索（前缀 / 状态 / 首字母 / 加入日期，支持两种排序）。
///
/// 前缀与首字母都落在 `normalized` 上，所以仍然走 `idx_words_norm`；
/// 「加入日期」是 `added_at` 的毫秒区间，不参与索引选择。
pub fn search_words(
    conn: &Connection,
    q: WordQuery<'_>,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    let effective = if !q.prefix.trim().is_empty() {
        q.prefix.trim().to_string()
    } else {
        q.letter
            .map(|c| c.to_string().to_lowercase())
            .unwrap_or_default()
    };
    let (lo, hi) = prefix_range(&effective);
    let offset = offset.max(0);
    let limit = limit.clamp(1, 200);

    // WHERE 的参数个数随「是否筛加入日期」变化，所以用 Value 列表按顺序传参
    let mut where_sql = String::from("normalized >= ? AND normalized < ?");
    let mut args: Vec<SqlValue> = vec![SqlValue::Text(lo), SqlValue::Text(hi)];

    let state_sql = q.state.sql();
    if state_sql != "1 = 1" {
        where_sql.push_str(&format!(" AND {state_sql}"));
    }
    if let Some(date) = q.added_on.filter(|d| !d.trim().is_empty()) {
        let (start, end) = sm2::day_range_ms(date, q.utc_offset_minutes)?;
        where_sql.push_str(" AND added_at >= ? AND added_at < ?");
        args.push(SqlValue::Integer(start));
        args.push(SqlValue::Integer(end));
    }

    let order = if q.newest_first {
        "added_at DESC, seq DESC"
    } else {
        "normalized ASC"
    };

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM words WHERE {where_sql}"),
            params_from_iter(args.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("统计词库失败: {e}"))?;

    let mut stmt = conn
        .prepare(&format!(
            "SELECT id, word, dit, state, due_at, added_at FROM words WHERE {where_sql} \
             ORDER BY {order} LIMIT ? OFFSET ?"
        ))
        .map_err(|e| format!("准备词库查询失败: {e}"))?;

    let mut page_args = args.clone();
    page_args.push(SqlValue::Integer(limit + 1));
    page_args.push(SqlValue::Integer(offset));

    let rows = stmt
        .query_map(params_from_iter(page_args.iter()), |row| {
            Ok(BrowseItem {
                id: row.get(0)?,
                text: row.get(1)?,
                gloss: row.get(2)?,
                state: row.get(3)?,
                due_at: row.get(4)?,
                added_at: row.get(5)?,
            })
        })
        .map_err(|e| format!("查询词库失败: {e}"))?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取词条失败: {e}"))?);
    }
    Ok(finish(items, total, limit))
}

/// 兼容入口：不筛加入日期、按字母升序（既有调用与测试都用它）。
pub fn browse_words(
    conn: &Connection,
    prefix: &str,
    state: StateFilter,
    letter: Option<char>,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    search_words(
        conn,
        WordQuery {
            prefix,
            state,
            letter,
            ..Default::default()
        },
        offset,
        limit,
    )
}

/// 句子本分页检索（包含匹配，含译文）。
pub fn browse_sentences(
    conn: &Connection,
    keyword: &str,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    let pattern = format!("%{}%", keyword.trim());
    let offset = offset.max(0);
    let limit = limit.clamp(1, 200);

    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sentences WHERE text LIKE ?1 OR IFNULL(trans, '') LIKE ?1",
            params![pattern],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("统计句子本失败: {e}"))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, text, trans, state, due_at, added_at FROM sentences \
             WHERE text LIKE ?1 OR IFNULL(trans, '') LIKE ?1 \
             ORDER BY seq DESC LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| format!("准备句子本查询失败: {e}"))?;

    let rows = stmt
        .query_map(params![pattern, limit + 1, offset], |row| {
            Ok(BrowseItem {
                id: row.get(0)?,
                text: row.get(1)?,
                gloss: row.get(2)?,
                state: row.get(3)?,
                due_at: row.get(4)?,
                added_at: row.get(5)?,
            })
        })
        .map_err(|e| format!("查询句子本失败: {e}"))?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取句子失败: {e}"))?);
    }
    let has_more = items.len() as i64 > limit;
    if has_more {
        items.truncate(limit as usize);
    }

    Ok(BrowsePage {
        items,
        total,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_sentence_record, apply_word_record};
    use crate::sm2::Grade;
    use serde_json::json;

    fn seed(conn: &Connection, words: &[(&str, &str)]) {
        for (i, (word, dit)) in words.iter().enumerate() {
            apply_word_record(
                conn,
                &json!({
                    "seq": i as i64 + 1,
                    "id": format!("id-{word}"),
                    "word": word,
                    "normalized": word.to_lowercase(),
                    "dit": dit,
                }),
            )
            .unwrap();
        }
    }

    #[test]
    fn empty_filters_return_everything_paginated() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(
            &conn,
            &[("apple", "苹果"), ("banana", "香蕉"), ("cherry", "樱桃")],
        );

        let page = browse_words(&conn, "", StateFilter::All, None, 0, 2).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 3);
        assert!(page.has_more);

        let page2 = browse_words(&conn, "", StateFilter::All, None, 2, 2).unwrap();
        assert_eq!(page2.items.len(), 1);
        assert!(!page2.has_more);
        // 分页不重叠
        assert_ne!(page.items[0].id, page2.items[0].id);
    }

    #[test]
    fn prefix_search_matches_only_that_prefix() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(
            &conn,
            &[("apple", "苹果"), ("apricot", "杏"), ("banana", "香蕉")],
        );

        let page = browse_words(&conn, "ap", StateFilter::All, None, 0, 10).unwrap();
        assert_eq!(page.total, 2);
        assert!(page.items.iter().all(|i| i.text.starts_with("ap")));

        // 大小写与首尾空格都应被规范化
        let upper = browse_words(&conn, "  AP  ", StateFilter::All, None, 0, 10).unwrap();
        assert_eq!(upper.total, 2);
    }

    #[test]
    fn letter_filter_works_like_prefix() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(&conn, &[("apple", "苹果"), ("banana", "香蕉")]);

        let page = browse_words(&conn, "", StateFilter::All, Some('B'), 0, 10).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].text, "banana");
    }

    #[test]
    fn state_filter_separates_new_from_learning() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(&conn, &[("apple", "苹果"), ("banana", "香蕉")]);
        crate::db::grade_word(&conn, "id-apple", Grade::Good, 1_760_000_000_000, 480).unwrap();

        let new = browse_words(&conn, "", StateFilter::New, None, 0, 10).unwrap();
        assert_eq!(new.total, 1);
        assert_eq!(new.items[0].text, "banana");

        let learning = browse_words(&conn, "", StateFilter::Review, None, 0, 10).unwrap();
        assert_eq!(learning.total, 1);
        assert_eq!(learning.items[0].text, "apple");
        assert_eq!(learning.items[0].state, 2);

        let mastered = browse_words(&conn, "", StateFilter::Mastered, None, 0, 10).unwrap();
        assert_eq!(mastered.total, 0);
    }

    /// 约束「搜索必须走索引」：几万词下 `LIKE '%kw%'` 会全表扫描，这里必须命中索引。
    #[test]
    fn browse_words_uses_index() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(&conn, &[("apple", "苹果")]);

        let (lo, hi) = prefix_range("ap");
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM words \
                 WHERE normalized >= ?1 AND normalized < ?2 ORDER BY normalized ASC",
                params![lo, hi],
                |row| row.get::<_, String>(3),
            )
            .unwrap();

        assert!(
            plan.contains("INDEX"),
            "前缀范围查询必须走索引，实际计划：{plan}"
        );
    }

    #[test]
    fn sentence_search_matches_text_and_translation() {
        let conn = crate::db::open_in_memory().unwrap();
        apply_sentence_record(
            &conn,
            &json!({"seq": 1, "id": "s1", "text": "the committee will reconvene", "trans": "委员会将重新开会", "added_at": 0}),
        )
        .unwrap();
        apply_sentence_record(
            &conn,
            &json!({"seq": 2, "id": "s2", "text": "it rains heavily", "trans": "雨下得很大", "added_at": 0}),
        )
        .unwrap();

        let by_text = browse_sentences(&conn, "committee", 0, 10).unwrap();
        assert_eq!(by_text.total, 1);
        assert_eq!(by_text.items[0].id, "s1");

        let by_trans = browse_sentences(&conn, "雨", 0, 10).unwrap();
        assert_eq!(by_trans.total, 1);
        assert_eq!(by_trans.items[0].id, "s2");

        let all = browse_sentences(&conn, "", 0, 10).unwrap();
        assert_eq!(all.total, 2);
    }

    #[test]
    fn limit_is_clamped_to_a_sane_range() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(&conn, &[("apple", "苹果")]);
        // limit = 0 会被夹到 1，而不是返回空页
        let page = browse_words(&conn, "", StateFilter::All, None, 0, 0).unwrap();
        assert_eq!(page.items.len(), 1);
    }

    /// 「按加入日期搜索」：只返回该日加入的词（本地日历日）。
    #[test]
    fn added_on_filter_returns_only_that_calendar_day() {
        const TZ: i64 = 480;
        let conn = crate::db::open_in_memory().unwrap();
        let (d11, _) = crate::sm2::day_range_ms("2026-10-11", TZ).unwrap();
        let (d12, _) = crate::sm2::day_range_ms("2026-10-12", TZ).unwrap();

        // 用本地中午，避开日界与午夜边界
        let seed_at = |word: &str, at: i64, seq: i64| {
            apply_word_record(
                &conn,
                &json!({"seq": seq, "id": format!("id-{word}"), "word": word,
                        "normalized": word, "dit": "x", "added_at": at}),
            )
            .unwrap();
        };
        seed_at("apple", d11 + 12 * 3_600_000, 1);
        seed_at("banana", d11 + 12 * 3_600_000, 2);
        seed_at("cherry", d12 + 12 * 3_600_000, 3);

        let q = WordQuery {
            added_on: Some("2026-10-11"),
            utc_offset_minutes: TZ,
            ..Default::default()
        };
        let page = search_words(&conn, q, 0, 50).unwrap();
        assert_eq!(page.total, 2);
        let mut words: Vec<&str> = page.items.iter().map(|i| i.text.as_str()).collect();
        words.sort();
        assert_eq!(words, vec!["apple", "banana"]);

        let q = WordQuery {
            added_on: Some("2026-10-12"),
            utc_offset_minutes: TZ,
            ..Default::default()
        };
        let page = search_words(&conn, q, 0, 50).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].text, "cherry");
    }

    /// 日期 + 前缀 + 状态是「与」的关系，不是「或」。
    #[test]
    fn added_on_combines_with_other_filters() {
        const TZ: i64 = 0;
        let conn = crate::db::open_in_memory().unwrap();
        let (d, _) = crate::sm2::day_range_ms("2026-10-11", TZ).unwrap();
        let seed_at = |word: &str, seq: i64| {
            apply_word_record(
                &conn,
                &json!({"seq": seq, "id": format!("id-{word}"), "word": word,
                        "normalized": word, "dit": "x", "added_at": d + 6 * 3_600_000}),
            )
            .unwrap();
        };
        seed_at("apple", 1);
        seed_at("apricot", 2);
        seed_at("banana", 3);
        crate::db::grade_word(&conn, "id-apple", Grade::Good, d + 7 * 3_600_000, TZ).unwrap();

        let q = WordQuery {
            prefix: "ap",
            state: StateFilter::Review,
            added_on: Some("2026-10-11"),
            utc_offset_minutes: TZ,
            ..Default::default()
        };
        let page = search_words(&conn, q, 0, 50).unwrap();
        assert_eq!(
            page.total, 1,
            "只有 apple 同时满足 前缀 ap + 复习中 + 当日加入"
        );
        assert_eq!(page.items[0].text, "apple");
    }

    /// 「按加入时间」排序：新加入的排前面。
    #[test]
    fn newest_first_sorts_by_added_time() {
        let conn = crate::db::open_in_memory().unwrap();
        let seed_at = |word: &str, at: i64, seq: i64| {
            apply_word_record(
                &conn,
                &json!({"seq": seq, "id": format!("id-{word}"), "word": word,
                        "normalized": word, "dit": "x", "added_at": at}),
            )
            .unwrap();
        };
        seed_at("apple", 1_000, 1);
        seed_at("banana", 3_000, 3);
        seed_at("cherry", 2_000, 2);

        let alpha = search_words(&conn, WordQuery::default(), 0, 50).unwrap();
        assert_eq!(
            alpha
                .items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>(),
            vec!["apple", "banana", "cherry"]
        );

        let q = WordQuery {
            newest_first: true,
            ..Default::default()
        };
        let newest = search_words(&conn, q, 0, 50).unwrap();
        assert_eq!(
            newest
                .items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>(),
            vec!["banana", "cherry", "apple"]
        );
    }

    /// 非法日期要报错，而不是静默返回全部（否则用户会以为筛选生效了）。
    #[test]
    fn bad_date_is_an_error() {
        let conn = crate::db::open_in_memory().unwrap();
        seed(&conn, &[("apple", "苹果")]);
        let q = WordQuery {
            added_on: Some("2026-13-01"),
            utc_offset_minutes: 0,
            ..Default::default()
        };
        assert!(search_words(&conn, q, 0, 10).is_err());
    }
}
