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

export interface Settings {
  pairing: Pairing | null;
  goals: DailyGoals;
  audio_accent: Accent;
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

/** 电脑端发布的新版本信息（`GET /api/app/latest`） */
export interface UpdateInfo {
  version: string;
  version_code: number;
  size: number;
  sha256: string;
  /** 远端版本是否高于本机 */
  newer: boolean;
}

/** 下载进度事件 `update://progress` 的载荷 */
export interface UpdateProgress {
  downloaded: number;
  total: number;
}
