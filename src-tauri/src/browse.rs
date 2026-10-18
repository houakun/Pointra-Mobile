//! 词库 / 句子本检索（P6 数据层）。
//!
//! 几万词规模下不能用 `LIKE '%kw%'`（全表扫描 + 无法走索引）。
//! 单词检索改用**前缀范围查询** `normalized >= lo AND normalized < hi`，
//! 它能命中 `idx_words_norm`（BINARY collation），由 `browse_words_uses_index` 测试锁死。
//!
//! 句子表没有 normalized 列，且句量远小于词量，因此句子搜索用包含匹配并接受全表扫描。

use crate::sm2;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection};
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
    added_on: Option<&str>,
    utc_offset_minutes: i64,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    let pattern = format!("%{}%", keyword.trim());
    let offset = offset.max(0);
    let limit = limit.clamp(1, 200);

    // 句子本一次只看一天（右上角 `‹ 日期 ›` 切换），所以日期是必选的筛选维度之一；
    // 关键词与日期可组合（「这一天里含某词的句子」）。
    let mut where_sql = String::from("(text LIKE ? OR IFNULL(trans, '') LIKE ?)");
    let mut args: Vec<SqlValue> = vec![SqlValue::Text(pattern.clone()), SqlValue::Text(pattern)];
    if let Some(date) = added_on.filter(|d| !d.trim().is_empty()) {
        // 与词库同一套做法：先把「本地日历日」换成毫秒区间，再按区间筛（可用索引、可单测）
        let (start, end) = sm2::day_range_ms(date, utc_offset_minutes)?;
        where_sql.push_str(" AND added_at >= ? AND added_at < ?");
        args.push(SqlValue::Integer(start));
        args.push(SqlValue::Integer(end));
    }

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM sentences WHERE {where_sql}"),
            params_from_iter(args.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("统计句子本失败: {e}"))?;

    let mut stmt = conn
        .prepare(&format!(
            // 按「加入时间」倒序：组内新的在上；没有 added_at 的老记录兜到 0 排最后。
            "SELECT id, text, trans, state, due_at, added_at FROM sentences WHERE {where_sql} \
             ORDER BY COALESCE(added_at, 0) DESC, seq DESC LIMIT ? OFFSET ?"
        ))
        .map_err(|e| format!("准备句子本查询失败: {e}"))?;

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
        .map_err(|e| format!("查询句子本失败: {e}"))?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("读取句子失败: {e}"))?);
    }
    Ok(finish(items, total, limit))
}

/// 最近一次收藏句子的「本地日历日」（`YYYY-MM-DD`）；一条都没有时返回 `None`。
///
/// 句子本打开时定位到这一天，而不是死盯「今天」——今天没收藏就会看到空白页。
pub fn latest_sentence_day(
    conn: &Connection,
    utc_offset_minutes: i64,
) -> Result<Option<String>, String> {
    let latest: Option<i64> = conn
        .query_row(
            // 0 / NULL 都是「没有时间」的老记录，不拿来做定位依据
            "SELECT MAX(added_at) FROM sentences WHERE added_at IS NOT NULL AND added_at > 0",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询句子日期失败: {e}"))?;
    Ok(latest.map(|ms| sm2::calendar_day(ms, utc_offset_minutes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_sentence_record, apply_word_record};
    use crate::sm2::Grade;
    use serde_json::json;

    /// 东八区。个别测 UTC 边界的用例会在函数内部重新定义 `TZ` 遮蔽它。
    const TZ: i64 = 480;

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
                params_from_iter([lo, hi]),
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

        let by_text = browse_sentences(&conn, "committee", None, TZ, 0, 10).unwrap();
        assert_eq!(by_text.total, 1);
        assert_eq!(by_text.items[0].id, "s1");

        let by_trans = browse_sentences(&conn, "雨", None, TZ, 0, 10).unwrap();
        assert_eq!(by_trans.total, 1);
        assert_eq!(by_trans.items[0].id, "s2");

        let all = browse_sentences(&conn, "", None, TZ, 0, 10).unwrap();
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

    /// 句子本必须按「加入时间」倒序：前端据此按日期分组，顺序错了分组就乱。
    /// 注：以前是 `seq DESC`，PC 下发记录的 seq 与 added_at 不成正比。
    #[test]
    fn sentences_are_ordered_by_added_at_desc() {
        let conn = crate::db::open_in_memory().unwrap();
        for (seq, id, text, added) in [
            (1_i64, "s-old", "old one", Some(1_600_000_000_000_i64)),
            (2, "s-new", "newest one", Some(1_760_000_000_000)),
            (3, "s-mid", "middle one", Some(1_700_000_000_000)),
            // 没有时间的记录（老数据）必须排最后，不能随机排在前面
            (4, "s-none", "unknown time", None),
        ] {
            let mut record = json!({"seq": seq, "id": id, "text": text});
            if let Some(added) = added {
                record["added_at"] = json!(added);
            }
            apply_sentence_record(&conn, &record).unwrap();
        }

        let page = browse_sentences(&conn, "", None, TZ, 0, 10).unwrap();
        let ids: Vec<&str> = page.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["s-new", "s-mid", "s-old", "s-none"],
            "应按下标时间倒序，未知时间排最后"
        );

        // 分页也不能把顺序打乱：第一页 2 条 + 第二页 2 条 = 完整顺序
        let first = browse_sentences(&conn, "", None, TZ, 0, 2).unwrap();
        let second = browse_sentences(&conn, "", None, TZ, 2, 2).unwrap();
        let paged: Vec<&str> = first
            .items
            .iter()
            .chain(second.items.iter())
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(paged, ids);
    }

    /// 句子本一次只看一天（`‹ 日期 ›` 切换）：按「本地日历日」换算成毫秒区间过滤，
    /// 且同一批数据在不同时区下归属的日期会不同。
    #[test]
    fn sentence_book_shows_one_calendar_day_at_a_time() {
        let conn = crate::db::open_in_memory().unwrap();
        let day = "2026-10-18";
        let (start, end) = crate::sm2::day_range_ms(day, TZ).unwrap();

        // 用 day_range_ms 算出的区间边界造数据：区间内两条、前后各一条
        for (seq, id, text, added) in [
            (1_i64, "s-in-1", "on that day morning", start + 1_000),
            (2, "s-in-2", "on that day night", end - 1_000),
            (3, "s-before", "the day before", start - 1_000),
            (4, "s-after", "the day after", end + 1_000),
        ] {
            apply_sentence_record(
                &conn,
                &json!({"seq": seq, "id": id, "text": text, "added_at": added}),
            )
            .unwrap();
        }

        let page = browse_sentences(&conn, "", Some(day), TZ, 0, 10).unwrap();
        let ids: Vec<&str> = page.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["s-in-2", "s-in-1"], "只显示当天的，且新的在前");
        assert_eq!(page.total, 2);

        // 关键词与日期可组合：当天 + 含 morning 的只有一条
        let filtered = browse_sentences(&conn, "morning", Some(day), TZ, 0, 10).unwrap();
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.items[0].id, "s-in-1");

        // 换一天
        let next = browse_sentences(&conn, "", Some("2026-10-19"), TZ, 0, 10).unwrap();
        let next_ids: Vec<&str> = next.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(next_ids, vec!["s-after"]);

        // 同样的记录，换 UTC 看：本地 08:00 之后那几条才落在 2026-10-18
        let utc = browse_sentences(&conn, "", Some(day), 0, 0, 10).unwrap();
        let utc_ids: Vec<&str> = utc.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            utc_ids,
            vec!["s-after", "s-in-2"],
            "时区变了，归属的日期也跟着变"
        );
    }

    /// 句子本打开时定位到「最近有句子的那天」，而不是死盯今天（今天没收藏就是空白页）。
    #[test]
    fn latest_sentence_day_points_at_the_newest_day() {
        let conn = crate::db::open_in_memory().unwrap();
        assert!(
            latest_sentence_day(&conn, TZ).unwrap().is_none(),
            "空句子本没有可定位的日期"
        );

        let (start, _) = crate::sm2::day_range_ms("2026-10-18", TZ).unwrap();
        apply_sentence_record(
            &conn,
            &json!({"seq": 1, "id": "a", "text": "older", "added_at": start}),
        )
        .unwrap();
        apply_sentence_record(
            &conn,
            &json!({"seq": 2, "id": "b", "text": "newer", "added_at": start + 3 * 86_400_000_i64}),
        )
        .unwrap();
        // 「没有时间」的老记录不该影响定位
        apply_sentence_record(&conn, &json!({"seq": 3, "id": "c", "text": "no time"})).unwrap();

        assert_eq!(
            latest_sentence_day(&conn, TZ).unwrap().as_deref(),
            Some("2026-10-21")
        );
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
