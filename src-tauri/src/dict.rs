//! 阅读查词 / 段落翻译：直连有道公开接口。
//!
//! 解析规则与 PC 端 `Pointra-Pub/src-tauri/src/utils/trans_server.rs` **字段语义对齐**：
//! 音标取 `usphone`/`ukphone`、释义按词性分组、`dit` 只留前 3 个义项、`wfs` 映射词形变化。
//! 但这里只保留移动端词卡需要的字段，不搬运 PC 前端那套 `TransData` 包装
//! （`translate`/`wordCard`/`voice` 三层壳对移动端没有意义）。
//!
//! ## 为什么走明文 http
//! 移动端 reqwest 是 `default-features = false`（**没有 TLS 后端**），
//! 加 rustls/openssl 会给 APK 增加数 MB，也违背「不引入重量级依赖」。
//! 实测有道 `jsonapi` 与 `aidemo/trans` 都直接接受明文 http（200，无跳转），
//! 而本 App 的 `network_security_config` 本来就放行明文（局域网同步需要），
//! 所以这里走 http。代价是查询不加密——查的是公开词典，不涉及账号与隐私。
//!
//! 这一层**不碰缓存**：缓存编排在 `commands` 里（用 `db::article_cache_*`），
//! 这样纯网络 + 解析逻辑可以在桌面上直接跑单测。

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::OnceLock;
use std::time::Duration;

const YOUDAO_JSON_API: &str = "http://dict.youdao.com/jsonapi";
const YOUDAO_FALLBACK_API: &str = "http://aidemo.youdao.com/trans";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 按词性分组的一组释义，对应前端词卡的「adj. 某一个；唯一的」这种行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DictGroup {
    pub pos: String,
    pub values: Vec<String>,
}

/// 词卡数据。`sentence`/`sentence_trans` 由调用方补上（见 `commands::lookup_word`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordLookup {
    pub word: String,
    pub phonetic_us: Option<String>,
    pub phonetic_uk: Option<String>,
    /// 主释义（最多 3 个义项，用「；」连接）
    pub dit: Option<String>,
    pub usual_dict: Vec<DictGroup>,
    /// 词形变化（复数/过去式…），键名与 PC 端 `Exchange` 一致
    pub exchange: Value,
    pub sentence: Option<String>,
    pub sentence_trans: Option<String>,
}

impl WordLookup {
    fn bare(word: &str, dit: String) -> Self {
        Self {
            word: word.to_string(),
            phonetic_us: None,
            phonetic_uk: None,
            dit: Some(dit),
            usual_dict: Vec::new(),
            exchange: json!({}),
            sentence: None,
            sentence_trans: None,
        }
    }
}

/// 全局安装 rustls 的 crypto provider（只装一次）。
///
/// Cargo.toml 用了 `rustls-no-provider`（为避开 Android 平台证书验证器的坑），
/// 这样 reqwest 在**构建任何 Client** 时都要求已全局装好 provider，
/// 否则直接 panic：「No rustls crypto provider is configured」。
/// 放在这里懒安装、而不是只在 App 启动时装：单测路径不经过 Tauri 的 `run()`。
pub(crate) fn ensure_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn shared_client() -> Result<&'static reqwest::Client, String> {
    ensure_crypto_provider();
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("创建网络客户端失败：{e}"))?;
    Ok(CLIENT.get_or_init(|| built))
}

// ---------------------------------------------------------------------------
// 对外接口
// ---------------------------------------------------------------------------

/// 查一个词的词典数据（纯网络，不读缓存）。
///
/// 降级顺序与 PC 端一致：词典接口 → 翻译接口 → 词典接口里的整句翻译。
pub async fn fetch_word(word: &str) -> Result<WordLookup, String> {
    fetch_word_at(word, YOUDAO_JSON_API, YOUDAO_FALLBACK_API).await
}

/// 接口地址可注入版本：只为了能在单测里把地址换成一个连不上的端口，
/// 从而真的验证「断网时给中文提示」（成功标准 6），而不是只靠读代码。
pub(crate) async fn fetch_word_at(
    word: &str,
    json_api: &str,
    fallback_api: &str,
) -> Result<WordLookup, String> {
    let word = word.trim();
    if word.is_empty() {
        return Err("查询内容为空".to_string());
    }
    let client = shared_client()?;
    let mut first_error: Option<String> = None;

    // 1. 词典接口：命中词条时数据最全（音标 / 词性分组 / 词形变化）
    match query_jsonapi(client, json_api, word).await {
        Ok(Some(payload)) => {
            if let Some(entry) = parse_dict_entry(word, &payload) {
                return Ok(entry);
            }
        }
        Ok(None) => {}
        Err(err) => first_error = Some(err),
    }

    // 2. 翻译接口：句子与未收录词条走这里
    match query_fallback(client, fallback_api, word).await {
        Ok(Some(payload)) => {
            if let Some(entry) = parse_fallback(word, &payload) {
                return Ok(entry);
            }
        }
        Ok(None) => {}
        Err(err) => first_error = first_error.or(Some(err)),
    }

    // 3. 最后兜底：词典接口响应里的双语例句译文
    match query_jsonapi(client, json_api, word).await {
        Ok(Some(payload)) => {
            if let Some(dit) = extract_sentence_translation(&payload) {
                return Ok(WordLookup::bare(word, dit));
            }
        }
        Ok(None) => {}
        Err(err) => first_error = first_error.or(Some(err)),
    }

    Err(match first_error {
        // 网络层错误要原样透出，才能让用户分清「没网」和「没这个词」
        Some(err) => err,
        None => "没有查到这个词的释义，换个词试试".to_string(),
    })
}

/// 翻译一段文本（段落或句子）。优先用翻译接口（整段译文最完整）。
pub async fn fetch_translation(text: &str) -> Result<String, String> {
    fetch_translation_at(text, YOUDAO_JSON_API, YOUDAO_FALLBACK_API).await
}

pub(crate) async fn fetch_translation_at(
    text: &str,
    json_api: &str,
    fallback_api: &str,
) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("待翻译内容为空".to_string());
    }
    let client = shared_client()?;
    let mut first_error: Option<String> = None;

    match query_fallback(client, fallback_api, text).await {
        Ok(Some(payload)) => {
            if let Some(translated) = parse_fallback_translation(&payload) {
                return Ok(translated);
            }
        }
        Ok(None) => {}
        Err(err) => first_error = Some(err),
    }

    match query_jsonapi(client, json_api, text).await {
        Ok(Some(payload)) => {
            if let Some(translated) = extract_sentence_translation(&payload) {
                return Ok(translated);
            }
        }
        Ok(None) => {}
        Err(err) => first_error = first_error.or(Some(err)),
    }

    Err(match first_error {
        Some(err) => err,
        None => "这段内容没能翻译出来，稍后再试".to_string(),
    })
}

// ---------------------------------------------------------------------------
// 网络层
// ---------------------------------------------------------------------------

fn build_url(base: &str, params: &[(&str, &str)]) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base).map_err(|e| format!("接口地址非法：{e}"))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

/// 把网络失败翻成人话（成功标准 6：断网要有明确中文提示，不能把英文堆栈丢给用户）。
///
/// 抽成纯函数是为了让三个分支都能被单测盖到：真实环境里「连接被拒」很难稳定复现
/// —— 开发机常挂 HTTP 代理，会把连不上的地址接成 502。
fn describe_network_failure(is_timeout: bool, is_connect: bool, raw: &str) -> String {
    if is_timeout {
        "查询超时了，检查一下网络再试".to_string()
    } else if is_connect {
        "连不上词典服务：手机可能没联网".to_string()
    } else {
        format!("网络请求失败：{raw}")
    }
}

/// 把 reqwest 的错误翻成人话：成功标准 6 要求断网时给出明确提示。
fn net_error(err: reqwest::Error) -> String {
    describe_network_failure(err.is_timeout(), err.is_connect(), &err.to_string())
}

async fn get_json(client: &reqwest::Client, url: reqwest::Url) -> Result<Value, String> {
    let resp = client
        .get(url)
        .header("Referer", "https://dict.youdao.com/")
        .send()
        .await
        .map_err(net_error)?;
    if !resp.status().is_success() {
        return Err(format!("词典服务返回 HTTP {}", resp.status()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("解析词典响应失败：{e}"))
}

async fn query_jsonapi(
    client: &reqwest::Client,
    base: &str,
    word: &str,
) -> Result<Option<Value>, String> {
    let url = build_url(base, &[("q", word)])?;
    get_json(client, url).await.map(Some)
}

async fn query_fallback(
    client: &reqwest::Client,
    base: &str,
    text: &str,
) -> Result<Option<Value>, String> {
    let url = build_url(base, &[("q", text), ("from", "auto"), ("to", "zh-CHS")])?;
    get_json(client, url).await.map(Some)
}

// ---------------------------------------------------------------------------
// 解析层
// ---------------------------------------------------------------------------

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 英文词条（`ec`）。中文词条（`ce`）不单独处理：
/// 阅读素材是英文文章，中文输入交给翻译接口兜底即可。
fn parse_dict_entry(word: &str, payload: &Value) -> Option<WordLookup> {
    let ec = payload.pointer("/ec/word/0")?;
    let simple = payload.pointer("/simple/word/0");
    let phone = |key: &str| -> Option<String> {
        str_field(ec, key)
            .or_else(|| simple.and_then(|s| str_field(s, key)))
            .map(str::to_string)
    };

    let mut groups: Vec<DictGroup> = Vec::new();
    let mut meanings: Vec<String> = Vec::new();
    if let Some(trs) = ec.get("trs").and_then(Value::as_array) {
        for entry in trs {
            let Some(items) = entry.pointer("/tr/0/l/i").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                // `trs[i].tr[0].l.i[j]` 正常是字符串；中文条目的 `ce` 分支才是对象
                let Some(raw) = item.as_str() else { continue };
                let (pos, meaning) = split_pos(raw.trim());
                if meaning.is_empty() {
                    continue;
                }
                push_group(&mut groups, &pos, &meaning);
                meanings.push(meaning);
            }
        }
    }
    if meanings.is_empty() {
        return None;
    }

    Some(WordLookup {
        word: word.to_string(),
        phonetic_us: phone("usphone"),
        phonetic_uk: phone("ukphone"),
        dit: Some(build_dit(&meanings)),
        usual_dict: groups,
        exchange: build_exchange(ec),
        sentence: None,
        sentence_trans: None,
    })
}

/// 翻译接口（`aidemo/trans`）：`basic.explains` 给释义，`translation` 给译文。
fn parse_fallback(word: &str, payload: &Value) -> Option<WordLookup> {
    if !fallback_ok(payload) {
        return None;
    }
    let dit = parse_fallback_translation(payload)?;
    let basic = payload.get("basic");

    let mut groups: Vec<DictGroup> = Vec::new();
    if let Some(explains) = basic
        .and_then(|b| b.get("explains"))
        .and_then(Value::as_array)
    {
        for item in explains {
            if let Some(text) = item.as_str() {
                let (pos, meaning) = split_pos(text.trim());
                if !meaning.is_empty() {
                    push_group(&mut groups, &pos, &meaning);
                }
            }
        }
    }

    let phone = |key: &str| -> Option<String> {
        basic
            .and_then(|b| b.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    Some(WordLookup {
        word: word.to_string(),
        phonetic_us: phone("us-phonetic"),
        phonetic_uk: phone("uk-phonetic"),
        dit: Some(dit),
        usual_dict: groups,
        exchange: json!({}),
        sentence: None,
        sentence_trans: None,
    })
}

fn fallback_ok(payload: &Value) -> bool {
    let code = match payload.get("errorCode") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => return false,
    };
    code == "0"
}

/// 翻译接口的译文：`translation` 数组。
fn parse_fallback_translation(payload: &Value) -> Option<String> {
    if !fallback_ok(payload) {
        return None;
    }
    let items = payload
        .get("translation")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| item.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if items.is_empty() {
        return None;
    }
    Some(items.join("；"))
}

/// 从词典响应里尽力提取整句翻译（与 PC 端同一条降级链）。
fn extract_sentence_translation(payload: &Value) -> Option<String> {
    if let Some(text) = payload.pointer("/fanyi/tran").and_then(Value::as_str) {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    if let Some(list) = payload
        .pointer("/web_trans/web-translation")
        .and_then(Value::as_array)
    {
        // 优先 `@same == "true"`（与原句完全匹配）
        for item in list {
            if item.get("@same").and_then(Value::as_str) == Some("true") {
                if let Some(text) = item.pointer("/trans/0/value").and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
        for item in list {
            if let Some(text) = item.pointer("/trans/0/value").and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    // 注意是 `sentence-translation`（中文）而不是 `sentence`（英文原句）
    if let Some(text) = payload
        .pointer("/blng_sents_part/sentence-pair/0/sentence-translation")
        .and_then(Value::as_str)
    {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    None
}

// ---------------------------------------------------------------------------
// 文本工具（与 PC 端 trans_server.rs 同源逻辑）
// ---------------------------------------------------------------------------

/// 拆分「词性 + 释义」，兼容 `v. xxx`、`vt.& vi. xxx`、`【名】 xxx`（无词性）等形式。
fn split_pos(raw: &str) -> (String, String) {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    let mut last_good = 0usize;

    loop {
        let mut probe = cursor;
        while probe < bytes.len() && (bytes[probe] == b' ' || bytes[probe] == b'&') {
            probe += 1;
        }
        let word_start = probe;
        while probe < bytes.len() && bytes[probe].is_ascii_alphabetic() {
            probe += 1;
        }
        // 必须「字母开头 + 紧跟一个点」才算词性标记，否则整体视为无词性
        if probe == word_start || probe >= bytes.len() || bytes[probe] != b'.' {
            break;
        }
        probe += 1;
        last_good = probe;
        cursor = probe;
    }

    if last_good == 0 {
        return (String::new(), raw.trim().to_string());
    }
    (
        raw[..last_good].trim().to_string(),
        raw[last_good..].trim().to_string(),
    )
}

fn push_group(groups: &mut Vec<DictGroup>, pos: &str, meaning: &str) {
    if let Some(entry) = groups.iter_mut().find(|g| g.pos == pos) {
        if !entry.values.iter().any(|v| v == meaning) {
            entry.values.push(meaning.to_string());
        }
    } else {
        groups.push(DictGroup {
            pos: pos.to_string(),
            values: vec![meaning.to_string()],
        });
    }
}

/// 主释义只留前 3 个义项（完整释义仍由 `usual_dict` 提供）。
fn build_dit(meanings: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for meaning in meanings {
        for segment in meaning.split(['；', ';']) {
            let segment = primary_sense(segment.trim());
            if segment.is_empty() || parts.iter().any(|p| p == segment) {
                continue;
            }
            parts.push(segment.to_string());
            if parts.len() >= 3 {
                break;
            }
        }
        if parts.len() >= 3 {
            break;
        }
    }
    parts.join("；")
}

/// 有道对短语常用「义项：补充解释」，主释义只留义项本身。
fn primary_sense(segment: &str) -> &str {
    if let Some((head, _)) = segment.split_once(['：', ':']) {
        let head = head.trim();
        if !head.is_empty() && head.chars().count() <= 15 {
            return head;
        }
    }
    segment
}

fn exchange_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "复数" => "word_pl",
        "第三人称单数" => "word_third",
        "现在分词" => "word_ing",
        "过去式" => "word_past",
        "过去分词" => "word_done",
        "原形" => "word_proto",
        _ => return None,
    })
}

/// 有道 `wfs`（中文词性名）→ 前端 `exchange`（英文键名）映射。
fn build_exchange(ec: &Value) -> Value {
    let mut map = Map::new();
    let Some(wfs) = ec.get("wfs").and_then(Value::as_array) else {
        return json!({});
    };

    for item in wfs {
        let name = item
            .pointer("/wf/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let value = item
            .pointer("/wf/value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if value.is_empty() {
            continue;
        }
        let Some(key) = exchange_key(name) else {
            continue;
        };

        let bucket = map
            .entry(key.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(arr) = bucket.as_array_mut() {
            if !arr.iter().any(|v| v.as_str() == Some(value)) {
                arr.push(Value::String(value.to_string()));
            }
        }
    }

    Value::Object(map)
}

// ---------------------------------------------------------------------------
// 缓存键
// ---------------------------------------------------------------------------

/// FNV-1a 64 位：缓存键只需要「同样的输入得到同样的键」，不需要密码学强度。
pub(crate) fn fnv1a_64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 单词缓存键：小写归一，`One` 与 `one` 共用一条缓存。
pub fn word_cache_key(word: &str) -> String {
    format!("w:{}", word.trim().to_lowercase())
}

/// 段落/句子缓存键：`<前缀>:<长度>-<hash>`。
/// 带上长度是为了让「不同长度但 hash 相同」不可能误命中。
pub fn text_cache_key(prefix: &str, text: &str) -> String {
    let text = text.trim();
    format!("{prefix}:{}-{:016x}", text.chars().count(), fnv1a_64(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD_APPLE: &str = r#"{
      "ec": {"word": [{
        "usphone": "ˈæp(ə)l", "ukphone": "ˈæp(ə)l",
        "trs": [{"tr": [{"l": {"i": ["n. 苹果；苹果树；苹果公司（Apple Inc.）"]}}]}],
        "wfs": [{"wf": {"name": "复数", "value": "apples"}}]
      }]},
      "simple": {"word": [{"usphone": "ˈæp(ə)l", "ukphone": "ˈæp(ə)l"}]}
    }"#;

    const WORD_RUN: &str = r#"{
      "ec": {"word": [{
        "usphone": "rʌn", "ukphone": "rʌn",
        "trs": [
          {"tr": [{"l": {"i": ["v. 跑，奔跑；参加（赛跑）"]} }]},
          {"tr": [{"l": {"i": ["n. 跑步，赛跑"]} }]},
          {"tr": [{"l": {"i": ["【名】 （Run）（塞）鲁恩（人名）"]} }]}
        ],
        "wfs": [
          {"wf": {"name": "复数", "value": "runs"}},
          {"wf": {"name": "过去式", "value": "ran"}},
          {"wf": {"name": "过去分词", "value": "run"}}
        ]
      }]}
    }"#;

    const SENTENCE_TRANS: &str = r#"{
      "fanyi": {"tran": "这些数字看起来令人瞩目。"}
    }"#;

    const FALLBACK_HELLO: &str = r#"{
      "errorCode": "0",
      "translation": ["你好", "喂"],
      "basic": {
        "us-phonetic": "həˈloʊ",
        "uk-phonetic": "həˈləʊ",
        "explains": ["int. 喂；哈罗", "n. 表示问候"]
      }
    }"#;

    #[test]
    fn parses_word_entry_with_phonetics_groups_and_exchange() {
        let payload: Value = serde_json::from_str(WORD_APPLE).unwrap();
        let entry = parse_dict_entry("apple", &payload).unwrap();

        assert_eq!(entry.phonetic_us.as_deref(), Some("ˈæp(ə)l"));
        assert_eq!(entry.phonetic_uk.as_deref(), Some("ˈæp(ə)l"));
        assert_eq!(entry.usual_dict.len(), 1);
        assert_eq!(entry.usual_dict[0].pos, "n.");
        assert_eq!(
            entry.usual_dict[0].values,
            vec!["苹果；苹果树；苹果公司（Apple Inc.）"]
        );
        // dit 只取前 3 个义项，按「；」拆开
        assert_eq!(
            entry.dit.as_deref(),
            Some("苹果；苹果树；苹果公司（Apple Inc.）")
        );
        assert_eq!(entry.exchange["word_pl"][0], "apples");
    }

    #[test]
    fn dit_keeps_at_most_three_senses() {
        let payload: Value = serde_json::from_str(WORD_RUN).unwrap();
        let entry = parse_dict_entry("run", &payload).unwrap();
        let dit = entry.dit.unwrap();
        assert_eq!(dit.split('；').count(), 3, "dit 最多 3 个义项：{dit}");
        // 完整释义仍在 usual_dict 里
        assert_eq!(entry.usual_dict.len(), 3);
    }

    #[test]
    fn parses_fallback_payload_into_card() {
        let payload: Value = serde_json::from_str(FALLBACK_HELLO).unwrap();
        let entry = parse_fallback("hello", &payload).unwrap();
        assert_eq!(entry.dit.as_deref(), Some("你好；喂"));
        assert_eq!(entry.phonetic_us.as_deref(), Some("həˈloʊ"));
        assert_eq!(entry.usual_dict.len(), 2);
        assert_eq!(entry.usual_dict[0].pos, "int.");
        assert_eq!(entry.usual_dict[0].values, vec!["喂；哈罗"]);
    }

    #[test]
    fn fallback_error_code_is_rejected() {
        let payload: Value = serde_json::from_str(r#"{"errorCode":"40"}"#).unwrap();
        assert!(parse_fallback("hello", &payload).is_none());
        assert!(parse_fallback_translation(&payload).is_none());
    }

    #[test]
    fn extract_sentence_translation_prefers_same_match() {
        let payload: Value = serde_json::from_str(SENTENCE_TRANS).unwrap();
        assert_eq!(
            extract_sentence_translation(&payload).as_deref(),
            Some("这些数字看起来令人瞩目。")
        );

        let web: Value = serde_json::from_str(
            r#"{"web_trans":{"web-translation":[
                 {"@same":"false","trans":[{"value":"次要"}]},
                 {"@same":"true","trans":[{"value":"精确匹配"}]}
               ]}}"#,
        )
        .unwrap();
        assert_eq!(
            extract_sentence_translation(&web).as_deref(),
            Some("精确匹配")
        );
    }

    #[test]
    fn split_pos_handles_common_shapes() {
        assert_eq!(split_pos("n. 苹果"), ("n.".into(), "苹果".into()));
        assert_eq!(
            split_pos("vt.& vi. 跑；奔跑"),
            ("vt.& vi.".into(), "跑；奔跑".into())
        );
        // 没有词性标记时整段当释义（`【名】` 这类中文标记不算）
        assert_eq!(
            split_pos("【名】 鲁恩"),
            (String::new(), "【名】 鲁恩".into())
        );
    }

    #[test]
    fn cache_keys_are_stable_and_case_insensitive() {
        assert_eq!(word_cache_key("One"), word_cache_key(" one "));
        let a = text_cache_key("p", "Hello world.");
        assert_eq!(a, text_cache_key("p", " Hello world. "));
        assert_ne!(a, text_cache_key("p", "Hello world!"));
        assert_ne!(a, text_cache_key("w", "Hello world."));
    }

    /// 成功标准 6 的「不崩溃 + 中文提示」：三个分支逐个盖住。
    #[test]
    fn network_failures_are_described_in_chinese() {
        let timeout = describe_network_failure(true, false, "operation timed out");
        assert!(timeout.contains("超时"), "{timeout}");

        let refused = describe_network_failure(false, true, "connection refused");
        assert!(refused.contains("连不上词典服务"), "{refused}");

        let other = describe_network_failure(false, false, "something odd");
        assert!(other.starts_with("网络请求失败"), "{other}");
    }

    /// 真实调不通时也必须返回可读中文（不 panic、不丢英文堆栈）。
    /// 具体措辞随网络环境而变（挂了代理时会变成 `HTTP 502`），所以只断言「是人话」。
    #[tokio::test]
    async fn unreachable_service_reports_readable_error() {
        let dead = "http://127.0.0.1:1/jsonapi";
        let hints = [
            "连不上词典服务",
            "查询超时",
            "网络请求失败",
            "词典服务返回 HTTP",
        ];

        let err = fetch_word_at("one", dead, "http://127.0.0.1:1/trans")
            .await
            .unwrap_err();
        assert!(
            hints.iter().any(|h| err.contains(h)),
            "错误信息要能看懂：{err}"
        );

        let err = fetch_translation_at("one two three", dead, "http://127.0.0.1:1/trans")
            .await
            .unwrap_err();
        assert!(
            hints.iter().any(|h| err.contains(h)),
            "错误信息要能看懂：{err}"
        );
    }

    /// 真实联网验证（默认跳过，手动跑：`cargo test --lib -- --ignored live_`）。
    /// 锁死「真实响应能解析出音标 + 按词性分组的释义 + 整段译文」。
    #[tokio::test]
    #[ignore = "需要联网：cargo test --lib -- --ignored live_"]
    async fn live_lookup_returns_phonetics_and_grouped_meanings() {
        let entry = fetch_word("significant").await.unwrap();
        assert!(
            entry.phonetic_us.is_some() || entry.phonetic_uk.is_some(),
            "真实响应里应该有音标"
        );
        assert!(
            !entry.usual_dict.is_empty(),
            "真实响应里应该有按词性分组的释义"
        );
        assert!(entry.dit.is_some(), "应该有主释义");
        assert!(entry.usual_dict.iter().all(|g| !g.values.is_empty()));
    }

    #[tokio::test]
    #[ignore = "需要联网：cargo test --lib -- --ignored live_"]
    async fn live_translation_returns_chinese() {
        let translated = fetch_translation("The figures appear striking.")
            .await
            .unwrap();
        assert!(!translated.trim().is_empty());
        assert!(
            translated
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "整段译文应该是中文：{translated}"
        );
    }
}
