// 与 Rust 命令层一一对应的类型定义。
// 字段名保持 snake_case：Tauri invoke 直接透传 serde 的默认命名，不做 camelCase 转换。

export interface Pairing {
  base: string;
  token: string;
  device_id: string;
  server_name?: string | null;
}

export interface DailyGoals {
  new_per_day: number;
  review_per_day: number;
}

export type Accent = "us" | "uk";

/** 主题：跟随系统 / 强制浅色 / 强制深色 */
export type Theme = "system" | "light" | "dark";

/**
 * 阅读页词卡出现位置。
 *
 * `auto` = 点词的左半屏就出右侧、右半屏就出左侧（不遮住正在读的那行）。
 */
export type WordCardSide = "auto" | "bottom" | "left" | "right";

export interface Settings {
  pairing: Pairing | null;
  goals: DailyGoals;
  audio_accent: Accent;
  theme: Theme;
  /** 阅读页词卡位置；旧版本 settings.json 没这个字段，读取时按 auto 兜底 */
  word_card_side?: WordCardSide;
  last_sync_at: number | null;
}

export interface PingInfo {
  name: string;
  version: string;
  device_id: string;
  paired: boolean;
}

export interface Overview {
  word_count: number;
  sentence_count: number;
  new_count: number;
  due_count: number;
  last_seq: number;
}

export type CardKind = "word" | "sentence";

export interface QueueEntry {
  kind: CardKind;
  id: string;
  seq: number;
  front: string;
  /** 进队时的状态：0=新卡，1/2=到期复习 */
  state: number;
}

export interface Progress {
  review_done: number;
  review_total: number;
  new_done: number;
  new_total: number;
  study_ms: number;
}

export interface CardDetail {
  kind: CardKind;
  id: string;
  front: string;
  trans: string | null;
  phonetic_us: string | null;
  phonetic_uk: string | null;
  audio_us: string | null;
  audio_uk: string | null;
  dit: string | null;
  usual_dict: unknown;
  /** 词形变化（复数/过去式/分词…）；句子为空对象 */
  exchange: unknown;
  state: number;
  due_at: number | null;
  added_at: number | null;
}

export interface BrowseItem {
  id: string;
  text: string;
  gloss: string | null;
  state: number;
  due_at: number | null;
  /** 加入时间（毫秒） */
  added_at: number | null;
}

export interface BrowsePage {
  items: BrowseItem[];
  total: number;
  has_more: boolean;
}

export interface SyncReport {
  words: number;
  sentences: number;
  last_seq: number;
  elapsed_ms: number;
}

export interface SyncProgress {
  phase: "words" | "sentences" | "done";
  applied: number;
  cursor: number;
  target: number;
}

export type StateFilter = "all" | "new" | "learning" | "review" | "mastered";

/** 电脑端 / 网络发布的新版本信息（`GET /api/app/latest` 或 GitHub Releases） */
export interface UpdateInfo {
  version: string;
  version_code: number;
  size: number;
  sha256: string;
  /** 这个版本改了什么（发布时写进元数据） */
  notes: string | null;
  /** 远端版本是否高于本机 */
  newer: boolean;
  /** 这份信息来自哪里：`online` = 网络（GitHub），`pc` = 电脑端 */
  source: string;
}

/** 下载进度事件 `update://progress` 的载荷 */
export interface UpdateProgress {
  downloaded: number;
  total: number;
}

// ---------------------------------------------------------------------------
// 阅读
// ---------------------------------------------------------------------------

/** 文章列表项（不含正文） */
export interface ArticleSummary {
  id: string;
  title: string;
  /** `builtin` = 内置素材；`import` = 自己粘贴导入 */
  source: string;
  /** 四六级核心词（只有内置素材有） */
  keywords: string[];
  /** 素材出处：真题写「某年某套真题 · 选自某刊」；导入文章为空 */
  origin: string;
  /** 考试类型：`cet4` / `cet6`；自写示例与导入文章为空串 */
  exam: string;
  /** 真题年份；自写示例与导入文章为 null */
  year: number | null;
  paragraph_count: number;
  created_at: number;
}

/** 文章详情：段落由 Rust 侧按规则切好（见 articles::split_paragraphs） */
export interface ArticleDetail {
  id: string;
  title: string;
  source: string;
  keywords: string[];
  origin: string;
  created_at: number;
  paragraphs: string[];
}

/** 按词性分组的一组释义 */
export interface DictGroup {
  pos: string;
  values: string[];
}

/**
 * 词卡数据（Rust `WordLookupResult`）。
 * `entry` 在 Rust 侧用了 `#[serde(flatten)]`，所以字段是摊平的。
 */
export interface WordLookup {
  word: string;
  phonetic_us: string | null;
  phonetic_uk: string | null;
  /** 主释义（最多 3 个义项） */
  dit: string | null;
  usual_dict: DictGroup[];
  exchange: Record<string, string[]>;
  /** 该词所在的句子（前端高亮用） */
  sentence: string | null;
  sentence_trans: string | null;
  /** 句中译文单独失败时的提示：词卡主体仍然可用 */
  sentence_error: string | null;
  /** 该词是否已在本机单词本里（决定星标实心/空心） */
  saved: boolean;
  /** 词典数据来自本地缓存（断网时 `true` 说明是读缓存） */
  cached: boolean;
}

export interface ParagraphTranslation {
  trans: string;
  cached: boolean;
}

/** `add_local_sentence` 的结果（阅读页选中句子后加入句子本） */
export interface LocalSentenceAdd {
  id: string;
  text: string;
  /** `true` = 这句本机已经收藏过（这次只刷新了译文） */
  existed: boolean;
}

/** `add_local_word` 的结果 */
export interface LocalWordAdd {
  id: string;
  word: string;
  /** `true` = 这个词本机已经收藏过，这次只刷新了词义 */
  existed: boolean;
}

/** 「本篇生词」的一条（`list_article_picks`） */
export interface ArticlePickWord {
  id: string;
  word: string;
  phonetic_us: string | null;
  phonetic_uk: string | null;
  dit: string | null;
  usual_dict: DictGroup[];
}

/** 「本篇生句」的一条 */
export interface ArticlePickSentence {
  id: string;
  text: string;
  trans: string | null;
}

/** 某篇文章里收藏过的生词与生句 */
export interface ArticlePicks {
  words: ArticlePickWord[];
  sentences: ArticlePickSentence[];
}
