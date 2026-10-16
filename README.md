# Pointra Mobile

与 [Pointra（PC 端）](https://github.com/houakun/Pointra-Pub) 通过**局域网同步**的 Android 背单词 App：
电脑上查到的单词/句子一键收藏，手机端按「不背单词式」卡片与简化 SM-2 复习。

> Tauri v2 Mobile + React 19 + Rust + SQLite。**PC 是唯一数据源**，同步协议单向只增。

## 功能

| 页面 | 说明 |
|---|---|
| 今日 | 今日复习 / 今日新词 / 学习时间；「开始学习」+「速记」两个入口 |
| 学习 | 单词卡与句子卡**共用同一队列**；「不认识」翻开释义、「我认识 / 太简单」前进；左右滑动切换上/下一张 |
| 速记 | 竖排单词快速过一遍：点词行内出释义（可同时展开多张）、左滑＝标记太简单、连续滚动 |
| 词库 | 前缀索引搜索 + 状态/首字母/**加入日期**筛选 + 排序切换；分页 + 虚拟滚动；点词进详情页 |
| 句子本 | 列表 / 搜索 / 单条删除；点进句子详情 |
| 阅读 | **四六级文章全文阅读**：**按 4/6 级 + 年份分类浏览**（筛选 chips 带计数 + 年份分组）；点任意单词弹词卡（音标 + 发音 + 按词性分组释义 + 所在句译文）、每段一键出/收译文；内置 **3 篇示例 + 140 篇四六级真题选文（四级 70 + 六级 70，2015–2026 全量）**（逐篇标明年份/套次与原文刊物），支持粘贴导入（多篇用 `---` 分隔可一次导入） |
| 设置 | 配对、每日上限、发音偏好、**应用更新**（应用内一键升级）、重置本机数据 |

- **简化 SM-2**：不认识 → 当日回队尾；我认识 → 间隔 1 → 6 → ×ease；太简单 → 毕业不再入队；日界 04:00
- **重复收藏**只刷新词义，**不重置学习进度**
- **阅读查词**：点词走「已配对时优先问电脑端（`GET /api/lookup`）→ 否则手机直连有道」，结果落本地缓存，
  同一个词/段不重复请求；断网时已查过的词与已翻译的段落仍可看（会明确提示网络失败，不会崩）
- **加入单词本**：阅读页收藏的词只存本机（`mobile-<hash>` id + 负 `seq`，与电脑端下发的记录互不覆盖），
  立即可背可复习；本轮不回传电脑端
- **阅读素材**：内置 143 篇（3 篇自写示例 + 140 篇四六级真题长篇阅读选文：四级 70 + 六级 70，
  即 2015–2026 年**全部** 140 套逻辑的选文，各约 1000–1250 词），逐篇在 front-matter 里标明年份/套次/原文刊物
  （如「2025 年 12 月英语六级真题 · 第 1 套（选自 The Guardian）」）。
  **素材清单由 `build.rs` 扫描 `src-tauri/assets/articles/*.md` 自动生成**：加一篇真题只需把 md 丢进那个目录，不用改 Rust 代码；
  批量抓取用 `node scripts/import-cet-articles.mjs --exam all [--year 2022]`（幂等，已导入的套次会跳过）；
  内置素材是**按 id 增量补插**的，已装 App 的用户下次启动就能拿到新文章（你手动删掉的不会自己复活）。
  **素材源：四六级真题阅读选文本身改编自外刊，版权归原作者/出版方所有，此处仅供个人背单词使用**。
  加文章：往 `src-tauri/assets/articles/` 放一个 md（front-matter: `id/title/source/keywords`）并在 `ARTICLE_FILES` 里加一行；
  内置素材是**按 id 增量补插**的，已装 App 的用户下次启动就能拿到新文章（你手动删掉的不会自己复活）。
- **应用内自更新**：从电脑端同步服务拉新版本 → 下载 → JNI 直接拉起系统安装器

## 安装

从 [Releases](https://github.com/houakun/Pointra-Mobile/releases) 下载 `Pointra-Mobile-<版本>-arm64.apk`：

1. 手机上点开安装包，首次需允许一次「安装未知应用」
2. 打开 App → **设置 → 手机同步** → 填电脑的局域网地址（如 `192.168.1.2:31415`）+ 6 位配对码
3. 电脑端需运行 Pointra 并在设置里打开「手机同步」，手机与电脑在同一 WiFi
   （电脑端首启会弹 Windows 防火墙提示，需勾选「专用网络」允许，或手动放行入站 TCP 31415）

要求 Android 8.0（API 26）及以上、arm64-v8a。

## 构建

```bash
pnpm install

# 真机用的 release 包（需先准备签名密钥，见 docs/02 §12）
bash scripts/build-apk.sh aarch64 release

# 模拟器用的 debug 包
bash scripts/build-apk.sh x86_64 debug
```

产物：`src-tauri/gen/android/app/build/outputs/apk/<abi>/<debug|release>/app-<abi>-<mode>.apk`

### 发布给手机端自更新

```powershell
powershell -ExecutionPolicy Bypass -File scripts/publish-update.ps1
```

把 APK 与版本元数据（versionCode / size / sha256）放进电脑端数据目录
`%APPDATA%\com.syjun.pointra\update\`，手机端「设置 → 应用更新」即可发现并一键升级。

## 目录

```
src/                  React 前端（6 个页面 + 速记页 + 单词详情页 + 阅读词卡组件）
src-tauri/assets/articles/  内置阅读素材（每篇一个 md，带 front-matter 出处；清单由 build.rs 自动生成）
scripts/import-cet-articles.mjs  批量抓取四六级真题选文（按年份/考试类型）
src-tauri/src/        Rust 数据层（不依赖 Tauri，可桌面单测）
  db.rs               SQLite：words / sentences / daily_stats / sync_state / articles / article_cache
  sm2.rs              简化 SM-2 + 04:00 日界
  session.rs          学习队列 + 今日统计
  browse.rs           词库/句子本分页检索（前缀走索引、加入日期筛选）
  articles.rs         阅读素材（段落切分规则 + 解析 `assets/articles/*.md`）
  dict.rs             阅读查词/段落翻译：有道解析 + 降级链 + 缓存键
  sync_client.rs      配对 / 增量拉取 / 查词转发
  sync_events.rs      WebSocket + 指数退避重连 + since 补拉
  commands.rs         Tauri 命令层（含阅读、收藏与自更新下载安装）
  settings.rs         本地设置（原子写）
src-tauri/gen/android Tauri 生成的 Android 工程（含明文 HTTP 放行、浅色主题、ApkInstaller.kt）
docs/                 需求与架构设计、Android 环境与构建踩坑记录
scripts/              build-apk.sh / publish-update.ps1
```

## 测试

```bash
cd src-tauri && cargo test --lib     # 79 passed（含 2 个需要联网、默认 --ignored 的用例）
cd src-tauri && cargo clippy --lib   # 0 warnings
npx tsc --noEmit                     # 通过
```

联网用例要手动跑：`cargo test --lib -- --ignored live_`（真实请求有道，验证解析与整段译文）。
PC 端同理：`cargo test --lib -- --ignored live_lookup_`（验证 `/api/lookup` 返回字段与手机直连一致）。

## 已知限制（诚实标注）

- **句子卡发音**受有道免费接口限制：`dictvoice` 是词库检索而非任意 TTS，
  单词发音必定可用，任意句子的整句发音会返回 500
- 手机本地删除的词，重装后会随全量拉取回来（单向同步的必然结果）
- WS 只在前台有效；后台靠「打开即补齐」，不做后台常驻推送
- 首次安装需手动装一次包；之后的版本可在 App 内一键更新

更多设计取舍见 [`docs/01-需求与架构设计.md`](docs/01-需求与架构设计.md)，
环境与构建踩坑见 [`docs/02-Android环境搭建清单.md`](docs/02-Android环境搭建清单.md)。
