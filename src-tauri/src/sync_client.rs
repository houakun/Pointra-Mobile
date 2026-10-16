//! 局域网同步客户端：只做「请求 + 解析」，落库交给 `db` 模块。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingInfo {
    pub name: String,
    pub version: String,
    pub device_id: String,
    pub paired: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairedInfo {
    pub token: String,
    pub device_id: String,
    #[serde(default)]
    pub server_name: Option<String>,
}

/// `GET /api/stats`：用于全量拉取时估算进度总量。
#[derive(Debug, Clone, Deserialize)]
pub struct ServerStats {
    pub word_count: i64,
    pub sentence_count: i64,
    pub last_seq: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    pub items: Vec<Value>,
    pub next_since: i64,
    pub has_more: bool,
}

pub struct SyncClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl SyncClient {
    /// 把用户输入的 `192.168.1.23:31415` / `http://10.0.2.2:31415` 规范化成完整 base。
    pub fn normalize_base(input: &str) -> String {
        let trimmed = input.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return String::new();
        }
        let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else {
            format!("http://{trimmed}")
        };
        // 没写端口时补默认端口（模拟器访问宿主机固定用 10.0.2.2）
        let has_port = with_scheme
            .split("://")
            .nth(1)
            .map(|rest| rest.contains(':'))
            .unwrap_or(false);
        if has_port {
            with_scheme
        } else {
            format!("{with_scheme}:31415")
        }
    }

    pub fn new(base: impl AsRef<str>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: Self::normalize_base(base.as_ref()),
            token: None,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn ping(&self) -> Result<PingInfo, String> {
        let resp = self
            .http
            .get(format!("{}/api/ping", self.base))
            .send()
            .await
            .map_err(|e| format!("无法连接电脑端：{e}"))?;
        decode(resp).await
    }

    pub async fn pair(&self, code: &str, device_name: &str) -> Result<PairedInfo, String> {
        let resp = self
            .http
            .post(format!("{}/api/pair", self.base))
            .json(&json!({ "code": code, "device_name": device_name }))
            .send()
            .await
            .map_err(|e| format!("无法连接电脑端：{e}"))?;
        decode(resp).await
    }

    pub async fn stats(&self) -> Result<ServerStats, String> {
        let resp = self
            .authed_get("/api/stats")
            .send()
            .await
            .map_err(|e| format!("无法连接电脑端：{e}"))?;
        decode(resp).await
    }

    /// 带 Bearer token 的 GET（`/api/ping`、`/api/pair` 之外的接口都要鉴权）。
    fn authed_get(&self, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.get(format!("{}{path}", self.base));
        match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }

    /// 增量拉取一页（`table` 取 `words` / `sentences`）。
    pub async fn fetch(&self, table: &str, since: i64, limit: i64) -> Result<Page, String> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| "尚未配对，缺少访问令牌".to_string())?;
        let resp = self
            .http
            .get(format!(
                "{}/api/{table}?since={since}&limit={limit}",
                self.base
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("拉取 {table} 失败：{e}"))?;
        decode(resp).await
    }

    /// 阅读页查词：`GET /api/lookup?q=<词>`（PC 端只读接口）。
    ///
    /// 返回的 JSON 与 `dict::WordLookup` 同形状，调用方可以直接反序列化。
    /// 它的价值：手机能连到 PC、但自己上不了外网时（热点 / 内网）依然能查词。
    pub async fn lookup(&self, word: &str) -> Result<Value, String> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| "尚未配对，缺少访问令牌".to_string())?;
        // 不用 `.query()`：移动端 reqwest 没开 `query` feature，
        // 而且词里可能带撇号（`company's`），必须走 URL 编码。
        let mut url = reqwest::Url::parse(&format!("{}/api/lookup", self.base))
            .map_err(|e| format!("接口地址非法：{e}"))?;
        url.query_pairs_mut().append_pair("q", word);
        let resp = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("无法连接电脑端：{e}"))?;
        decode(resp).await
    }
}

async fn decode<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, String> {
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败：{e}"))?;

    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(body);
        return Err(format!("电脑端返回 {status}：{detail}"));
    }

    serde_json::from_str::<T>(&body).map_err(|e| format!("解析响应失败：{e}"))
}

/// 配对信息（本地持久化，见设置页）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Pairing {
    pub base: String,
    pub token: String,
    pub device_id: String,
    pub server_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_handles_common_inputs() {
        assert_eq!(
            SyncClient::normalize_base("10.0.2.2:31415"),
            "http://10.0.2.2:31415"
        );
        assert_eq!(
            SyncClient::normalize_base("  http://192.168.1.23:31415/  "),
            "http://192.168.1.23:31415"
        );
        // 省略端口 → 补默认 31415
        assert_eq!(
            SyncClient::normalize_base("192.168.1.23"),
            "http://192.168.1.23:31415"
        );
        assert_eq!(SyncClient::normalize_base(""), "");
    }

    #[test]
    fn page_parses_server_payload() {
        let raw = r#"{"items":[{"seq":1,"word":"scout"}],"next_since":1,"has_more":true}"#;
        let page: Page = serde_json::from_str(raw).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next_since, 1);
        assert!(page.has_more);
    }

    #[test]
    fn paired_info_tolerates_missing_server_name() {
        let info: PairedInfo = serde_json::from_str(r#"{"token":"t","device_id":"d"}"#).unwrap();
        assert_eq!(info.token, "t");
        assert!(info.server_name.is_none());
    }

    #[test]
    fn server_stats_parses_payload() {
        let s: ServerStats =
            serde_json::from_str(r#"{"word_count":12000,"sentence_count":300,"last_seq":12300}"#)
                .unwrap();
        assert_eq!(s.word_count, 12000);
        assert_eq!(s.sentence_count, 300);
        assert_eq!(s.last_seq, 12300);
    }

    #[test]
    fn pairing_roundtrips_through_json() {
        let pairing = Pairing {
            base: "http://10.0.2.2:31415".into(),
            token: "abc".into(),
            device_id: "dev-1".into(),
            server_name: Some("Pointra".into()),
        };
        let text = serde_json::to_string(&pairing).unwrap();
        let back: Pairing = serde_json::from_str(&text).unwrap();
        assert_eq!(back.base, pairing.base);
        assert_eq!(back.token, pairing.token);
    }
}
