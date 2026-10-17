//! Tauri 命令层：把纯 Rust 数据层暴露给前端，并管理 App 生命周期内的状态。

use crate::browse::{self, BrowsePage, StateFilter};
use crate::db;
use crate::dict;
use crate::session::{self, DailyGoals, Progress, QueueEntry};
use crate::settings::{self, Settings};
use crate::sm2::{self, Grade};
use crate::sync_client::{PingInfo, SyncClient};
use crate::sync_events::{self, EventSession};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub settings: Arc<Mutex<Settings>>,
    pub settings_path: PathBuf,
    pub events: Arc<Mutex<Option<EventSession>>>,
}

/// 在阻塞线程池里访问 SQLite：async 命令持有 `MutexGuard` 会破坏 `Send`，
/// 而且 sqlite 是同步 API，放主线程会卡 UI。
async fn blocking<T, F>(db: Arc<Mutex<Connection>>, f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().map_err(|e| format!("数据库锁失败：{e}"))?;
        f(&conn)
    })
    .await
    .map_err(|e| format!("后台任务失败：{e}"))?
}

// ---------------------------------------------------------------------------
// 设置
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let s = state
        .settings
        .lock()
        .map_err(|e| format!("设置锁失败：{e}"))?;
    Ok(s.clone())
}

/// 只允许前端改「每日上限 / 发音偏好」；配对信息必须走 pair / unpair。
#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    let mut guard = state
        .settings
        .lock()
        .map_err(|e| format!("设置锁失败：{e}"))?;
    let mut next = guard.clone();
    next.goals = settings.goals;
    next.audio_accent = settings.audio_accent;
    next.theme = settings.theme;
    settings::save(&state.settings_path, &next)?;
    *guard = next.clone();
    Ok(next)
}

// ---------------------------------------------------------------------------
// 统计
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub word_count: i64,
    pub sentence_count: i64,
    pub new_count: i64,
    pub due_count: i64,
    pub last_seq: i64,
}

#[tauri::command]
pub async fn get_overview(state: State<'_, AppState>) -> Result<Overview, String> {
    let db = state.db.clone();
    let now = settings::now_ms();
    blocking(db, move |conn| {
        let st = db::stats(conn, now)?;
        Ok(Overview {
            word_count: st.word_count,
            sentence_count: st.sentence_count,
            new_count: st.new_count,
            due_count: st.due_count,
            last_seq: db::get_last_seq(conn),
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// 配对与同步
// ---------------------------------------------------------------------------

fn current_pairing(state: &State<'_, AppState>) -> Result<(String, String), String> {
    let s = state
        .settings
        .lock()
        .map_err(|e| format!("设置锁失败：{e}"))?;
    let p = s
        .pairing
        .clone()
        .ok_or_else(|| "尚未配对，请先在设置页填写 PC 地址与配对码".to_string())?;
    Ok((p.base, p.token))
}

#[tauri::command]
pub async fn probe_server(base: String) -> Result<PingInfo, String> {
    SyncClient::new(&base).ping().await
}

#[tauri::command]
pub async fn pair_server(
    state: State<'_, AppState>,
    base: String,
    code: String,
    device_name: String,
) -> Result<Settings, String> {
    let client = SyncClient::new(&base);
    // 先探活：地址写错时能给出「无法连接电脑端」这种可读错误，而不是配对码错误
    let _ = client.ping().await?;
    if code.trim().is_empty() {
        return Err("配对码不能为空".into());
    }
    let info = client.pair(code.trim(), device_name.trim()).await?;

    let mut guard = state
        .settings
        .lock()
        .map_err(|e| format!("设置锁失败：{e}"))?;
    guard.pairing = Some(crate::sync_client::Pairing {
        base: SyncClient::normalize_base(&base),
        token: info.token,
        device_id: info.device_id,
        server_name: info.server_name.or_else(|| Some("Pointra".into())),
    });
    guard.last_sync_at = None;
    settings::save(&state.settings_path, &guard)?;
    Ok(guard.clone())
}

#[tauri::command]
pub async fn unpair_server(state: State<'_, AppState>) -> Result<Settings, String> {
    if let Some(session) = state
        .events
        .lock()
        .map_err(|e| format!("事件锁失败：{e}"))?
        .take()
    {
        session.shutdown();
    }

    let mut guard = state
        .settings
        .lock()
        .map_err(|e| format!("设置锁失败：{e}"))?;
    guard.pairing = None;
    guard.last_sync_at = None;
    settings::save(&state.settings_path, &guard)?;
    Ok(guard.clone())
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub words: i64,
    pub sentences: i64,
    pub last_seq: i64,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
struct SyncProgress {
    phase: String,
    applied: i64,
    cursor: i64,
    target: i64,
}

/// 增量/全量补拉。
///
/// 两表共用全局 `seq`，所以必须**各自从同一个起始游标完整拉完**
/// （不能拉完 words 就推高游标，否则会漏掉夹在中间 seq 的 sentences）。
/// 最后取两者终值的 max 作为新游标。
#[tauri::command]
pub async fn sync_now(app: AppHandle, state: State<'_, AppState>) -> Result<SyncReport, String> {
    let (base, token) = current_pairing(&state)?;
    let db = state.db.clone();
    let settings_arc = state.settings.clone();
    let settings_path = state.settings_path.clone();

    let client = SyncClient::new(&base).with_token(token);
    let server = client.stats().await?;
    let target = server.last_seq;
    let start = Instant::now();

    let base_cursor = blocking(db.clone(), |conn| Ok(db::get_last_seq(conn))).await?;

    async fn drain(
        app: &AppHandle,
        db: &Arc<Mutex<Connection>>,
        client: &SyncClient,
        table: &'static str,
        from: i64,
        target: i64,
    ) -> Result<(i64, i64), String> {
        let mut cursor = from;
        let mut applied = 0i64;
        loop {
            let page = client.fetch(table, cursor, 1000).await?;
            let count = page.items.len() as i64;
            applied += count;

            if count > 0 {
                let items = page.items.clone();
                let dbc = db.clone();
                let is_word = table == "words";
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let conn = dbc.lock().map_err(|e| format!("数据库锁失败：{e}"))?;
                    for item in &items {
                        if is_word {
                            db::apply_word_record(&conn, item)?;
                        } else {
                            db::apply_sentence_record(&conn, item)?;
                        }
                    }
                    Ok(())
                })
                .await
                .map_err(|e| format!("落库任务失败：{e}"))??;
            }

            cursor = page.next_since;
            let _ = app.emit(
                "sync://progress",
                SyncProgress {
                    phase: table.to_string(),
                    applied,
                    cursor,
                    target,
                },
            );

            // count == 0 但 has_more == true 属异常响应，直接跳出避免死循环
            if !page.has_more || count == 0 {
                break;
            }
        }
        Ok((cursor, applied))
    }

    let (word_cursor, words_applied) =
        drain(&app, &db, &client, "words", base_cursor, target).await?;
    let (sentence_cursor, sentences_applied) =
        drain(&app, &db, &client, "sentences", base_cursor, target).await?;

    let final_cursor = base_cursor.max(word_cursor).max(sentence_cursor);
    blocking(db.clone(), move |conn| db::set_last_seq(conn, final_cursor)).await?;

    let stamp = settings::now_ms();
    {
        let mut guard = settings_arc
            .lock()
            .map_err(|e| format!("设置锁失败：{e}"))?;
        guard.last_sync_at = Some(stamp);
        settings::save(&settings_path, &guard)?;
    }

    let _ = app.emit(
        "sync://progress",
        SyncProgress {
            phase: "done".into(),
            applied: words_applied + sentences_applied,
            cursor: final_cursor,
            target,
        },
    );

    Ok(SyncReport {
        words: words_applied,
        sentences: sentences_applied,
        last_seq: final_cursor,
        elapsed_ms: start.elapsed().as_millis() as i64,
    })
}

// ---------------------------------------------------------------------------
// 词库 / 句子本
// ---------------------------------------------------------------------------

fn parse_state(state: &str) -> StateFilter {
    match state {
        "new" => StateFilter::New,
        "learning" => StateFilter::Learning,
        "review" => StateFilter::Review,
        "mastered" => StateFilter::Mastered,
        _ => StateFilter::All,
    }
}

/// 词库检索入参。
///
/// 参数一多就超了 clippy 的 `too_many_arguments`，而且 IPC 上传一个对象
/// 比一长串扁平参数更好改；前端相应地传 `{ q: {...}, offset, limit }`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordQueryArgs {
    #[serde(default)]
    pub prefix: String,
    /// 任意/未知值都按「全部」处理（见 `parse_state`）
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub letter: Option<String>,
    /// 加入日期（本地日历日 `YYYY-MM-DD`），空串/None = 不筛
    #[serde(default)]
    pub added_on: Option<String>,
    /// true = 按加入时间倒序
    #[serde(default)]
    pub newest_first: bool,
    #[serde(default)]
    pub utc_offset_minutes: i64,
}

#[tauri::command]
pub async fn browse_words(
    app_state: State<'_, AppState>,
    q: WordQueryArgs,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    let db = app_state.db.clone();
    let filter = parse_state(&q.state);
    let letter = q.letter.and_then(|s| s.chars().next());
    blocking(db, move |conn| {
        browse::search_words(
            conn,
            browse::WordQuery {
                prefix: &q.prefix,
                state: filter,
                letter,
                added_on: q.added_on.as_deref().filter(|d| !d.trim().is_empty()),
                utc_offset_minutes: q.utc_offset_minutes,
                newest_first: q.newest_first,
            },
            offset,
            limit,
        )
    })
    .await
}

#[tauri::command]
pub async fn browse_sentences(
    state: State<'_, AppState>,
    keyword: String,
    offset: i64,
    limit: i64,
) -> Result<BrowsePage, String> {
    let db = state.db.clone();
    blocking(db, move |conn| {
        browse::browse_sentences(conn, &keyword, offset, limit)
    })
    .await
}

#[tauri::command]
pub async fn delete_sentence(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.clone();
    blocking(db, move |conn| {
        let n = conn
            .execute("DELETE FROM sentences WHERE id = ?1", params![id])
            .map_err(|e| format!("删除句子失败：{e}"))?;
        if n == 0 {
            return Err("该句子不存在".into());
        }
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// 阅读文章（本地功能，与同步无关）
// ---------------------------------------------------------------------------
//
// 文章是纯本机数据：不进 `words`/`sentences`，不参与同步，也不影响 PC 端。

#[tauri::command]
pub async fn list_articles(state: State<'_, AppState>) -> Result<Vec<db::ArticleSummary>, String> {
    let db = state.db.clone();
    blocking(db, db::list_articles).await
}

#[tauri::command]
pub async fn get_article(
    state: State<'_, AppState>,
    id: String,
) -> Result<db::ArticleDetail, String> {
    let db = state.db.clone();
    blocking(db, move |conn| {
        db::get_article(conn, &id)?.ok_or_else(|| format!("文章不存在：{id}"))
    })
    .await
}

#[tauri::command]
pub async fn save_article(
    state: State<'_, AppState>,
    title: String,
    body: String,
) -> Result<db::ArticleDetail, String> {
    let db = state.db.clone();
    blocking(db, move |conn| db::save_article(conn, &title, &body)).await
}

/// 删除单篇文章。前端负责二次确认（这里只保证「一次调用只删一个 id」）。
#[tauri::command]
pub async fn delete_article(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.clone();
    blocking(db, move |conn| db::delete_article(conn, &id)).await
}

// ---------------------------------------------------------------------------
// 阅读：查词与段落翻译（带本地缓存）
// ---------------------------------------------------------------------------

/// 缓存优先：命中就直接返回，未命中才调用 `fetch`。
///
/// 抽成独立函数是为了让「同一个词/段不重复发请求」这条约束能被单测锁死
/// —— 它是成功标准 3 与「缓存命中」契约的直接落点。
pub(crate) async fn cache_first<F, Fut>(
    cached: Option<Value>,
    fetch: F,
) -> Result<(Value, bool), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    if let Some(hit) = cached {
        return Ok((hit, true));
    }
    Ok((fetch().await?, false))
}

// 查词**不依赖电脑端**：直连有道（`dict::fetch_word`）。
//
// 曾经做过「已配对时优先问 PC 的 /api/lookup」，但那样点词能不能出释义
// 就取决于「电脑是否开机 + 是否配对 + 是否同一 WiFi」—— 实际用起来就是「查不了词」。
// 现在纯走手机自己的网络，没联网时给明确的中文提示即可。
// （PC 端的 /api/lookup 接口与 `sync_client::lookup` 都保留着，将来要用再说。）

/// 取词条：本机缓存 → 直连有道；成功后写缓存。
///
/// 返回的 `bool` 表示是否命中本机缓存（前端据它显示「释义来自本机缓存」）。
async fn resolve_word_entry(
    state: &State<'_, AppState>,
    word: &str,
) -> Result<(dict::WordLookup, bool), String> {
    let db = state.db.clone();
    let key = dict::word_cache_key(word);
    let cached = {
        let key = key.clone();
        blocking(db.clone(), move |conn| {
            Ok(db::article_cache_get(conn, &key))
        })
        .await?
    };

    if let Some(hit) = cached {
        let entry: dict::WordLookup =
            serde_json::from_value(hit).map_err(|e| format!("词条数据损坏：{e}"))?;
        return Ok((entry, true));
    }

    let entry = dict::fetch_word(word).await?;

    let data = serde_json::to_value(&entry).map_err(|e| format!("序列化词条失败：{e}"))?;
    blocking(db, move |conn| {
        db::article_cache_put(conn, &key, "word", &data)
    })
    .await?;

    Ok((entry, false))
}

#[derive(Debug, Clone, Serialize)]
pub struct WordLookupResult {
    #[serde(flatten)]
    pub entry: dict::WordLookup,
    /// 该词所在的句子（前端高亮用）
    pub sentence: Option<String>,
    pub sentence_trans: Option<String>,
    /// 句译文单独失败时的提示：此时词卡本身仍然可用，不应该整张卡报错
    pub sentence_error: Option<String>,
    /// 该词是否已在本机单词本里（决定词卡星标是实心还是空心）
    pub saved: bool,
    /// 词典数据是否来自本地缓存（断网时 cached=true 就是「读缓存仍可用」的证据）
    pub cached: bool,
}

/// 查一个词：词典数据 + 它在文中所属句子的译文。
#[tauri::command]
pub async fn lookup_word(
    state: State<'_, AppState>,
    word: String,
    sentence: Option<String>,
) -> Result<WordLookupResult, String> {
    let word = word.trim().to_string();
    if word.is_empty() {
        return Err("没有选中单词".to_string());
    }
    let sentence = sentence
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let db = state.db.clone();

    // --- 1. 词典部分（按词缓存，与所在句子无关；已配对时优先问 PC）---
    let (entry, cached_hit) = resolve_word_entry(&state, &word).await?;

    // --- 2. 句中译文（按句子缓存：同一个词在不同句子里不能互相串）---
    let mut sentence_trans = None;
    let mut sentence_error = None;
    if let Some(sentence) = sentence.clone() {
        let key = dict::text_cache_key("s", &sentence);
        let cached = {
            let key = key.clone();
            blocking(db.clone(), move |conn| {
                Ok(db::article_cache_get(conn, &key))
            })
            .await?
        };
        match cache_first(cached, || async {
            dict::fetch_translation(&sentence).await.map(|t| json!(t))
        })
        .await
        {
            Ok((value, hit)) => {
                if !hit {
                    let data = value.clone();
                    let key = key.clone();
                    blocking(db.clone(), move |conn| {
                        db::article_cache_put(conn, &key, "sentence", &data)
                    })
                    .await?;
                }
                sentence_trans = value.as_str().map(str::to_string);
            }
            // 句译文失败只降级，不让整张词卡失败
            Err(err) => sentence_error = Some(err),
        }
    }

    let saved = {
        let word = word.clone();
        blocking(db.clone(), move |conn| db::has_local_word(conn, &word)).await?
    };

    Ok(WordLookupResult {
        entry,
        sentence,
        sentence_trans,
        sentence_error,
        saved,
        cached: cached_hit,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ParagraphTranslation {
    pub trans: String,
    pub cached: bool,
}

/// 翻译一整段（「译」按钮）。
#[tauri::command]
pub async fn translate_paragraph(
    state: State<'_, AppState>,
    text: String,
) -> Result<ParagraphTranslation, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("这一段没有内容".to_string());
    }

    let db = state.db.clone();
    let key = dict::text_cache_key("p", &text);
    let cached = {
        let key = key.clone();
        blocking(db.clone(), move |conn| {
            Ok(db::article_cache_get(conn, &key))
        })
        .await?
    };

    let (value, hit) = cache_first(cached, || async {
        dict::fetch_translation(&text).await.map(|t| json!(t))
    })
    .await?;

    if !hit {
        let data = value.clone();
        blocking(db.clone(), move |conn| {
            db::article_cache_put(conn, &key, "paragraph", &data)
        })
        .await?;
    }

    let trans = value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "翻译结果格式异常".to_string())?;
    Ok(ParagraphTranslation { trans, cached: hit })
}

// ---------------------------------------------------------------------------
// 阅读：本机收藏
// ---------------------------------------------------------------------------

/// 把阅读页查到的词加进本机词库。
///
/// 只收一个 `word`：词典字段从**同一条查词缓存**里取，前端不搬运、也无法伪造词义。
#[tauri::command]
pub async fn add_local_word(
    state: State<'_, AppState>,
    word: String,
) -> Result<db::LocalWordAdd, String> {
    let word = word.trim().to_string();
    if word.is_empty() {
        return Err("没有选中单词".to_string());
    }

    let db = state.db.clone();
    let (entry, _) = resolve_word_entry(&state, &word).await?;
    blocking(db, move |conn| db::add_local_word(conn, &word, &entry)).await
}

/// 当前 App 版本（编译进二进制的包版本，来自 `tauri.conf.json`）。
///
/// 前端原本调 `@tauri-apps/api/app` 的 `getVersion()`，但那条路要过 ACL 权限，
/// 而一旦被拦就只能静默失败 —— 界面会退回到写死的旧版本号，
/// 看起来就是「版本号不对」。自己暴露一个命令最稳，也绕不过去。
#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

// ---------------------------------------------------------------------------
// 学习
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CardDetail {
    pub kind: String,
    pub id: String,
    pub front: String,
    pub trans: Option<String>,
    pub phonetic_us: Option<String>,
    pub phonetic_uk: Option<String>,
    pub audio_us: Option<String>,
    pub audio_uk: Option<String>,
    pub dit: Option<String>,
    pub usual_dict: Value,
    /// 词形变化（复数/过去式/分词…）。有道 `exchange` 字段原样下发。
    pub exchange: Value,
    pub state: i32,
    pub due_at: Option<i64>,
    pub added_at: Option<i64>,
}

fn word_card(conn: &Connection, id: &str) -> Result<Option<CardDetail>, String> {
    conn.query_row(
        "SELECT id, word, dit, usual_dict, exchange, phonetic_us, phonetic_uk, audio_us, \
         audio_uk, state, due_at, added_at FROM words WHERE id = ?1",
        params![id],
        |row| {
            let usual: Option<String> = row.get(3)?;
            let exchange: Option<String> = row.get(4)?;
            Ok(CardDetail {
                kind: "word".into(),
                id: row.get(0)?,
                front: row.get(1)?,
                trans: None,
                dit: row.get(2)?,
                usual_dict: usual
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| json!([])),
                exchange: exchange
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| json!({})),
                phonetic_us: row.get(5)?,
                phonetic_uk: row.get(6)?,
                audio_us: row.get(7)?,
                audio_uk: row.get(8)?,
                state: row.get(9)?,
                due_at: row.get(10)?,
                added_at: row.get(11)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取单词卡失败：{e}"))
}

fn sentence_card(conn: &Connection, id: &str) -> Result<Option<CardDetail>, String> {
    conn.query_row(
        "SELECT id, text, trans, state, due_at, added_at FROM sentences WHERE id = ?1",
        params![id],
        |row| {
            Ok(CardDetail {
                kind: "sentence".into(),
                id: row.get(0)?,
                front: row.get(1)?,
                trans: row.get(2)?,
                phonetic_us: None,
                phonetic_uk: None,
                audio_us: None,
                audio_uk: None,
                dit: None,
                usual_dict: json!([]),
                exchange: json!({}),
                state: row.get(3)?,
                due_at: row.get(4)?,
                added_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取句子卡失败：{e}"))
}

#[tauri::command]
pub async fn get_card(
    state: State<'_, AppState>,
    kind: String,
    id: String,
) -> Result<CardDetail, String> {
    let db = state.db.clone();
    blocking(db, move |conn| {
        let card = if kind == "sentence" {
            sentence_card(conn, &id)?
        } else {
            word_card(conn, &id)?
        };
        card.ok_or_else(|| format!("卡片不存在：{id}"))
    })
    .await
}

#[tauri::command]
pub async fn build_queue(state: State<'_, AppState>) -> Result<Vec<QueueEntry>, String> {
    let db = state.db.clone();
    let goals = daily_goals(&state)?;
    let now = settings::now_ms();
    blocking(db, move |conn| session::build_queue(conn, now, goals)).await
}

#[tauri::command]
pub async fn get_progress(
    state: State<'_, AppState>,
    utc_offset_minutes: i64,
) -> Result<Progress, String> {
    let db = state.db.clone();
    let goals = daily_goals(&state)?;
    let now = settings::now_ms();
    blocking(db, move |conn| {
        session::today_progress(conn, now, utc_offset_minutes, goals)
    })
    .await
}

fn daily_goals(state: &State<'_, AppState>) -> Result<DailyGoals, String> {
    let s = state
        .settings
        .lock()
        .map_err(|e| format!("设置锁失败：{e}"))?;
    Ok(s.goals)
}

/// `grade`：0=不认识 / 1=我认识 / 2=标记为太简单（与 `sm2::Grade` 一一对应）。
#[tauri::command]
pub async fn grade_card(
    state: State<'_, AppState>,
    kind: String,
    id: String,
    grade: i32,
    was_new: bool,
    utc_offset_minutes: i64,
) -> Result<(), String> {
    let table = match kind.as_str() {
        "word" => "words",
        "sentence" => "sentences",
        other => return Err(format!("未知卡片类型：{other}")),
    };
    let grade = match grade {
        0 => Grade::Again,
        1 => Grade::Good,
        2 => Grade::TooEasy,
        other => return Err(format!("未知评分：{other}")),
    };
    let db = state.db.clone();
    let now = settings::now_ms();
    blocking(db, move |conn| {
        session::record_grade(conn, table, &id, grade, now, utc_offset_minutes, was_new)
    })
    .await
}

#[tauri::command]
pub async fn add_study_ms(
    state: State<'_, AppState>,
    ms: i64,
    utc_offset_minutes: i64,
) -> Result<(), String> {
    if ms <= 0 {
        return Ok(());
    }
    let day = sm2::day_key(settings::now_ms(), utc_offset_minutes);
    let db = state.db.clone();
    blocking(db, move |conn| db::add_study_ms(conn, &day, ms)).await
}

// ---------------------------------------------------------------------------
// 实时推送
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn start_events(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let (base, token) = current_pairing(&state)?;
    let mut guard = state
        .events
        .lock()
        .map_err(|e| format!("事件锁失败：{e}"))?;
    if let Some(old) = guard.take() {
        old.shutdown();
    }
    *guard = Some(sync_events::spawn(app, state.db.clone(), base, token));
    Ok(())
}

#[tauri::command]
pub async fn stop_events(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(session) = state
        .events
        .lock()
        .map_err(|e| format!("事件锁失败：{e}"))?
        .take()
    {
        session.shutdown();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 重置本机数据
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn reset_all(state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.clone();
    blocking(db, move |conn| {
        conn.execute_batch(
            "DELETE FROM words; DELETE FROM sentences; DELETE FROM daily_stats; \
             DELETE FROM sync_state; DELETE FROM articles; DELETE FROM article_cache;",
        )
        .map_err(|e| format!("重置失败：{e}"))?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 详情页靠 `usual_dict` 渲染释义、靠 `exchange` 渲染「词形变化」，
    /// 这两个字段必须真的下发（之前 `get_card` 漏了 exchange，前端读了空）。
    #[test]
    fn word_card_exposes_usual_dict_and_exchange() {
        let conn = db::open_in_memory().unwrap();
        db::apply_word_record(
            &conn,
            &json!({
                "seq": 1, "id": "w1", "word": "apple", "normalized": "apple",
                "dit": "n. 苹果",
                "usual_dict": [{"pos": "n.", "values": ["苹果", "苹果公司"]}],
                "exchange": {"word_pl": ["apples"]},
                "added_at": 1_760_000_000_000i64
            }),
        )
        .unwrap();

        let card = word_card(&conn, "w1").unwrap().unwrap();
        assert_eq!(card.front, "apple");
        assert_eq!(card.dit.as_deref(), Some("n. 苹果"));
        assert_eq!(card.usual_dict[0]["pos"], "n.");
        assert_eq!(card.exchange["word_pl"][0], "apples");
    }

    /// 服务端只发 version/version_code/size/sha256，`newer` 是客户端算的：
    /// 少了 `#[serde(default)]` 就会整包解不出来（端到端实测踩过）。
    #[test]
    fn update_info_parses_server_payload_without_newer() {
        let raw = r#"{"version":"0.2.1","version_code":2001,"size":9761445,"sha256":"abc"}"#;
        let info: UpdateInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.version, "0.2.1");
        assert_eq!(info.version_code, 2001);
        assert!(!info.newer);
        assert!(parse_version(&info.version) > parse_version("0.2.0"));
    }

    /// 版本比较：自更新靠它判断「远端是否更新」，不能拿字符串直接比。
    #[test]
    fn version_compare_is_numeric_not_lexicographic() {
        assert!(parse_version("0.3.0") > parse_version("0.2.9"));
        // 字符串比较会把 "0.2.10" 判成小于 "0.2.9"，数值比较才对
        assert!(parse_version("0.2.10") > parse_version("0.2.9"));
        assert_eq!(parse_version("0.2"), (0, 2, 0));
        assert_eq!(parse_version("1.2.3"), (1, 2, 3));
        assert_eq!(parse_version("garbage"), (0, 0, 0));
        assert!(!(parse_version("0.2.0") > parse_version("0.2.0")));
    }

    #[test]
    fn sentence_card_has_empty_exchange() {
        let conn = db::open_in_memory().unwrap();
        db::apply_sentence_record(
            &conn,
            &json!({
                "seq": 2, "id": "s1", "text": "one two three",
                "trans": "一二三", "added_at": 0
            }),
        )
        .unwrap();

        let card = sentence_card(&conn, "s1").unwrap().unwrap();
        assert_eq!(card.front, "one two three");
        assert_eq!(card.trans.as_deref(), Some("一二三"));
        assert!(card
            .exchange
            .as_object()
            .map(|o| o.is_empty())
            .unwrap_or(false));
    }
}

// ---------------------------------------------------------------------------
// 手机端自更新
//
// 两个来源，**默认先试公网**（不依赖电脑开机/配对），失败再回落电脑端：
// - 公网：GitHub Releases 的 `latest` 固定地址（发布时把 pointra.json / pointra.apk 传成附件）；
// - 电脑端：局域网同步服务的 /api/app/latest（内网更快，但要求电脑开着且已配对）。
// ---------------------------------------------------------------------------

/// 公网更新源：GitHub Releases 的固定地址（`latest` 永远指向最新 release）。
///
/// 只要每次发布把这两个文件作为 release 附件上传，就不需要客户端改代码。
/// 注：国内网络访问 GitHub 可能不稳，所以电脑端通道保留着当备选。
const ONLINE_META_URL: &str =
    "https://github.com/houakun/Pointra-Mobile/releases/latest/download/pointra.json";
const ONLINE_APK_URL: &str =
    "https://github.com/houakun/Pointra-Mobile/releases/latest/download/pointra.apk";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub version_code: i64,
    pub size: u64,
    pub sha256: String,
    /// 这个版本改了什么（发布时写进元数据，手机端直接展示给用户）。
    /// 服务端可能没写，所以 `default`。
    #[serde(default)]
    pub notes: Option<String>,
    /// 远端版本是否高于本机（false = 放的是同版本或更旧）。
    /// **服务端不下发这个字段**，由客户端算，所以必须 `default`，
    /// 否则反序列化会报 “missing field `newer`”。
    #[serde(default)]
    pub newer: bool,
    /// 这份信息来自哪里：`online`（GitHub）/ `pc`（电脑端）。客户端自己填。
    #[serde(default)]
    pub source: String,
}

/// 把 `x.y.z` 拆成可比较的数字（缺段/非数字按 0，不 panic）。
fn parse_version(v: &str) -> (u64, u64, u64) {
    let mut parts = v.trim().split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// 带 UA 的客户端：GitHub 的接口/附件会拒绝没有 User-Agent 的请求。
fn update_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("Pointra-Mobile/uptate-check")
        .build()
        .map_err(|e| format!("创建网络客户端失败：{e}"))
}

/// 公网源查版本（GitHub Releases）。`Ok(None)` = 还没发布过 release。
async fn check_update_online(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let resp = update_client()?
        .get(ONLINE_META_URL)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("连不上更新服务器：{e}"))?;

    // 404 = 仓库还没发过 release，或附件名不对（不算错误，直接回落电脑端）
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("更新服务器返回 {}", resp.status()));
    }

    let mut info: UpdateInfo = resp
        .json()
        .await
        .map_err(|e| format!("解析版本信息失败：{e}"))?;
    info.newer = parse_version(&info.version) > parse_version(&app.package_info().version.to_string());
    info.source = "online".to_string();
    Ok(Some(info))
}

/// 问电脑端有没有新版本。`Ok(None)` = 电脑端还没发布（http 404）。
async fn check_update_via_pc(
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<Option<UpdateInfo>, String> {
    let (base, _) = current_pairing(state)?;
    let url = format!("{}/api/app/latest", base.trim_end_matches('/'));
    let resp = update_client()?
        .get(&url)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| format!("无法连接电脑端：{e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("电脑端返回 {}", resp.status()));
    }
    let mut info: UpdateInfo = resp
        .json()
        .await
        .map_err(|e| format!("解析版本信息失败：{e}"))?;
    let local = app.package_info().version.to_string();
    info.newer = parse_version(&info.version) > parse_version(&local);
    info.source = "pc".to_string();
    Ok(Some(info))
}

/// 查新版本：**先公网（不依赖电脑），再电脑端（内网更快）**。
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<UpdateInfo>, String> {
    let online = check_update_online(&app).await;
    match &online {
        // 公网已经有结论（有新版/同版本）就直接用
        Ok(Some(_)) => return online,
        // 公网没发过 release → 回落电脑端
        Ok(None) => {}
        // 公网失败（没网 / GitHub 不通）→ 回落电脑端，两边都记下来
        Err(_) => {}
    }

    let via_pc = check_update_via_pc(&app, &state).await;
    match (online, via_pc) {
        (_, Ok(Some(info))) => Ok(Some(info)),
        (_, Ok(None)) => Ok(None),
        // 电脑端也失败：把公网的失败原因一起报出来，方便判断是「没网」还是「没发布」
        (Err(online_err), Err(pc_err)) => Err(format!("更新检查失败：{online_err}；电脑端：{pc_err}")),
        (_, Err(pc_err)) => Err(pc_err),
    }
}

/// 从 `url` 流式下载 APK 到缓存目录，边下边发进度事件。
///
/// `who` 只用于错误提示（“更新服务器”/“电脑端”），便于用户判断是网络还是电脑的问题。
async fn download_apk_from(app: &AppHandle, url: &str, who: &str) -> Result<String, String> {
    use futures_util::StreamExt;
    use std::io::Write;

    let resp = update_client()?
        .get(url)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("{who}下载失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{who}返回 {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);

    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("取缓存目录失败：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建缓存目录失败：{e}"))?;
    let dest = dir.join("pointra-update.apk");

    let mut file = std::fs::File::create(&dest).map_err(|e| format!("创建文件失败：{e}"))?;
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断：{e}"))?;
        file.write_all(&chunk)
            .map_err(|e| format!("写入失败：{e}"))?;
        downloaded += chunk.len() as u64;
        let _ = app.emit(
            "update://progress",
            json!({ "downloaded": downloaded, "total": total }),
        );
    }
    file.flush().map_err(|e| format!("写入失败：{e}"))?;
    Ok(dest.to_string_lossy().to_string())
}

/// 下载 APK 到应用缓存目录（该目录已被 manifest 里的 FileProvider 暴露），
/// 返回本地路径——由前端再交给系统安装器。
///
/// 与 `check_update` 一致：**先公网，失败再回落电脑端**。
#[tauri::command]
pub async fn download_update(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    match download_apk_from(&app, ONLINE_APK_URL, "更新服务器").await {
        Ok(path) => Ok(path),
        Err(online_err) => {
            // 公网不行（没网 / GitHub 不通 / 还没上传附件）→ 试电脑端
            let Ok((base, _)) = current_pairing(&state) else {
                return Err(online_err);
            };
            let url = format!("{}/api/app/download", base.trim_end_matches('/'));
            download_apk_from(&app, &url, "电脑端")
                .await
                .map_err(|pc_err| format!("{online_err}；电脑端：{pc_err}"))
        }
    }
}

// ---------------------------------------------------------------------------
// 拉起系统安装器（B1：JNI 调自己写的 Kotlin 助手，不走 ACL）
// ---------------------------------------------------------------------------

/// 把已下载的 APK 交给 Android 系统安装器。
///
/// 关键坑：native 线程上 `env.find_class` 用的是**系统** ClassLoader，找不到 App 自己的类
/// （会报 ClassNotFoundException），必须拿 Activity 的 ClassLoader 去 `loadClass`。
#[cfg(target_os = "android")]
fn install_apk_via_system(path: &str) -> Result<(), String> {
    use jni::objects::{JClass, JObject, JValue};
    use jni::JavaVM;

    let ctx = ndk_context::android_context();
    let vm =
        unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| format!("JNI 初始化失败: {e}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach 当前线程失败: {e}"))?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    // 1) 从 Activity 拿 ClassLoader
    let loader = env
        .call_method(
            &activity,
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("取 ClassLoader 失败: {e}"))?;

    // 2) 用它的 loadClass 拿我们的类
    let class_name = env
        .new_string("com.syjun.pointra.mobile.ApkInstaller")
        .map_err(|e| format!("构造类名失败: {e}"))?;
    let class_name_obj = JObject::from(class_name);
    let class_obj = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&class_name_obj)],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("加载 ApkInstaller 失败: {e}"))?;
    let class = JClass::from(class_obj);

    // 3) 调 ApkInstaller.install(Activity, String)
    let path_j = env
        .new_string(path)
        .map_err(|e| format!("构造路径失败: {e}"))?;
    let path_obj = JObject::from(path_j);
    env.call_static_method(
        class,
        "install",
        "(Landroid/app/Activity;Ljava/lang/String;)V",
        &[JValue::Object(&activity), JValue::Object(&path_obj)],
    )
    .map_err(|e| format!("调用 ApkInstaller.install 失败: {e}"))?;
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn install_apk_via_system(_path: &str) -> Result<(), String> {
    Err("安装 APK 只在 Android 上支持".into())
}

/// 拉起系统安装器安装已下载的 APK（前端在 `download_update` 之后调用）。
#[tauri::command]
pub async fn install_update(path: String) -> Result<(), String> {
    // JNI 调用是同步阻塞的，放到后台线程池，别阻塞 UI 线程
    tokio::task::spawn_blocking(move || install_apk_via_system(&path))
        .await
        .map_err(|e| format!("后台任务失败：{e}"))?
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法获取应用数据目录：{e}"))?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建数据目录失败：{e}"))?;

            let db_path = dir.join("vocab.db");
            let conn = db::open(&db_path)?;
            // 内置阅读素材只在首次启动写一次（标记存在 sync_state 里）
            db::seed_builtin_articles(&conn)?;
            let settings_path = dir.join("settings.json");

            app.manage(AppState {
                db: Arc::new(Mutex::new(conn)),
                settings: Arc::new(Mutex::new(settings::load(&settings_path))),
                settings_path,
                events: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            get_overview,
            probe_server,
            pair_server,
            unpair_server,
            sync_now,
            browse_words,
            browse_sentences,
            delete_sentence,
            list_articles,
            get_article,
            save_article,
            delete_article,
            lookup_word,
            translate_paragraph,
            add_local_word,
            app_version,
            get_card,
            build_queue,
            get_progress,
            grade_card,
            add_study_ms,
            start_events,
            stop_events,
            reset_all,
            check_update,
            download_update,
            install_update,
        ])
        .run(tauri::generate_context!())
        .expect("Pointra Mobile 启动失败");
}
