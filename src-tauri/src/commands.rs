//! Tauri 命令层：把纯 Rust 数据层暴露给前端，并管理 App 生命周期内的状态。

use crate::browse::{self, BrowsePage, StateFilter};
use crate::db;
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
pub async fn sync_now(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SyncReport, String> {
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

    let (word_cursor, words_applied) = drain(&app, &db, &client, "words", base_cursor, target).await?;
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
    pub state: i32,
    pub due_at: Option<i64>,
    pub added_at: Option<i64>,
}

fn word_card(conn: &Connection, id: &str) -> Result<Option<CardDetail>, String> {
    conn.query_row(
        "SELECT id, word, dit, usual_dict, phonetic_us, phonetic_uk, audio_us, audio_uk, \
         state, due_at, added_at FROM words WHERE id = ?1",
        params![id],
        |row| {
            let usual: Option<String> = row.get(3)?;
            Ok(CardDetail {
                kind: "word".into(),
                id: row.get(0)?,
                front: row.get(1)?,
                trans: None,
                dit: row.get(2)?,
                usual_dict: usual
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| json!([])),
                phonetic_us: row.get(4)?,
                phonetic_uk: row.get(5)?,
                audio_us: row.get(6)?,
                audio_uk: row.get(7)?,
                state: row.get(8)?,
                due_at: row.get(9)?,
                added_at: row.get(10)?,
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
             DELETE FROM sync_state;",
        )
        .map_err(|e| format!("重置失败：{e}"))?;
        Ok(())
    })
    .await
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
            get_card,
            build_queue,
            get_progress,
            grade_card,
            add_study_ms,
            start_events,
            stop_events,
            reset_all,
        ])
        .run(tauri::generate_context!())
        .expect("Pointra Mobile 启动失败");
}
