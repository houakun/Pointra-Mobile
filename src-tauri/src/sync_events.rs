//! PC 端 WebSocket 推送的客户端（P5）。
//!
//! 真实边界（需求文档 §4.4）：Android 会冻结后台进程，WS 只在前台有效。
//! 所以这里不追求「后台常驻」，而是：
//! - 前台长连接，收到 `*.added` 立即入库 + 通知前端；
//! - 断线按 1s→2s→4s…上限 30s 指数退避重连；
//! - 重连成功后由前端触发一次 `sync_now` 补拉，补齐断线期间错过的记录。

use crate::db;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use futures_util::StreamExt;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

pub struct EventSession {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl EventSession {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.abort();
    }
}

/// 把 `http://10.0.2.2:31415` 变成 `ws://10.0.2.2:31415/api/events?token=…`
pub fn ws_url(base: &str, token: &str) -> String {
    let ws = base
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{ws}/api/events?token={token}")
}

/// 收到一条事件：先落库（保持本地与 PC 一致），再通知前端刷新。
fn handle_event(app: &AppHandle, db: &Arc<Mutex<Connection>>, event: &Value) {
    let kind = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let seq = event.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);

    let applied = (|| -> Result<(), String> {
        let conn = db.lock().map_err(|e| format!("数据库锁失败: {e}"))?;
        match kind {
            "word.added" | "word.updated" => db::apply_word_record(&conn, &payload)?,
            "sentence.added" | "sentence.updated" => db::apply_sentence_record(&conn, &payload)?,
            // 未知类型忽略，保持向前兼容
            _ => return Ok(()),
        }
        if seq > db::get_last_seq(&conn) {
            db::set_last_seq(&conn, seq)?;
        }
        Ok(())
    })();

    if let Err(err) = applied {
        let _ = app.emit("sync://error", json!({ "message": err }));
        return;
    }
    let _ = app.emit("sync://added", json!({ "type": kind, "seq": seq }));
}

/// 启动前台 WS 会话。已存在时由调用方先 shutdown。
pub fn spawn(
    app: AppHandle,
    db: Arc<Mutex<Connection>>,
    base: String,
    token: String,
) -> EventSession {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_task = stop.clone();

    let handle = tokio::spawn(async move {
        let url = ws_url(&base, &token);
        let mut backoff = 1u64;

        while !stop_task.load(Ordering::SeqCst) {
            if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(&url).await {
                backoff = 1;
                // 连上（含重连）就通知前端补拉一次，覆盖断线期间错过的记录
                let _ = app.emit("sync://reconnected", ());

                while !stop_task.load(Ordering::SeqCst) {
                    match ws.next().await {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            if let Ok(event) = serde_json::from_str::<Value>(&text) {
                                handle_event(&app, &db, &event);
                            }
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                            break;
                        }
                        Some(Err(_)) => break,
                        // Ping/Pong/Binary 一律忽略（服务端用 Ping 做保活）
                        Some(Ok(_)) => {}
                    }
                }
            }

            if stop_task.load(Ordering::SeqCst) {
                break;
            }
            let _ = app.emit("sync://disconnected", ());
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });

    EventSession { stop, handle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_converts_http_and_https() {
        assert_eq!(
            ws_url("http://10.0.2.2:31415", "abc"),
            "ws://10.0.2.2:31415/api/events?token=abc"
        );
        assert_eq!(
            ws_url("https://192.168.1.5:31415", "t"),
            "wss://192.168.1.5:31415/api/events?token=t"
        );
    }
}
