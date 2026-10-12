import { invoke } from "@tauri-apps/api/core";
import type {
  BrowsePage,
  CardDetail,
  CardKind,
  Overview,
  PingInfo,
  Progress,
  QueueEntry,
  Settings,
  StateFilter,
  SyncReport,
  UpdateInfo,
} from "./types";

/**
 * Rust 标准库拿不到本地时区，SM-2 的 04:00 日界依赖这个偏移，
 * 所以由前端每次调用时传入（东八区 = 480）。
 */
function tzOffset(): number {
  return -new Date().getTimezoneOffset();
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  resetAll: () => invoke<void>("reset_all"),

  getOverview: () => invoke<Overview>("get_overview"),

  probeServer: (base: string) => invoke<PingInfo>("probe_server", { base }),
  pairServer: (base: string, code: string, deviceName: string) =>
    invoke<Settings>("pair_server", { base, code, deviceName }),
  unpairServer: () => invoke<Settings>("unpair_server"),
  syncNow: () => invoke<SyncReport>("sync_now"),

  browseWords: (
    prefix: string,
    state: StateFilter,
    letter: string | null,
    addedOn: string | null,
    newestFirst: boolean,
    offset: number,
    limit: number,
  ) =>
    invoke<BrowsePage>("browse_words", {
      q: {
        prefix,
        state,
        letter,
        addedOn,
        newestFirst,
        utcOffsetMinutes: tzOffset(),
      },
      offset,
      limit,
    }),
  browseSentences: (keyword: string, offset: number, limit: number) =>
    invoke<BrowsePage>("browse_sentences", { keyword, offset, limit }),
  deleteSentence: (id: string) => invoke<void>("delete_sentence", { id }),

  getCard: (kind: CardKind, id: string) => invoke<CardDetail>("get_card", { kind, id }),
  buildQueue: () => invoke<QueueEntry[]>("build_queue"),
  gradeCard: (kind: CardKind, id: string, grade: number, wasNew: boolean) =>
    invoke<void>("grade_card", {
      kind,
      id,
      grade,
      wasNew,
      utcOffsetMinutes: tzOffset(),
    }),
  addStudyMs: (ms: number) => invoke<void>("add_study_ms", { ms, utcOffsetMinutes: tzOffset() }),
  getProgress: () => invoke<Progress>("get_progress", { utcOffsetMinutes: tzOffset() }),

  startEvents: () => invoke<void>("start_events"),
  stopEvents: () => invoke<void>("stop_events"),

  /** 问电脑端有无新版本；`null` = 电脑端还没发布 */
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  /** 下载 APK 到应用缓存目录，返回本地路径（再交给系统安装器） */
  downloadUpdate: () => invoke<string>("download_update"),
  /** 拉起系统安装器安装已下载的 APK（JNI 调 Kotlin 助手，不走 ACL） */
  installUpdate: (path: string) => invoke<void>("install_update", { path }),
};

/** 有道在线发音 URL（R3）。Android 端只用 HTML5 audio 播放，不移植 rodio。 */
export function youdaoAudio(word: string, accent: "us" | "uk"): string {
  const type = accent === "uk" ? 1 : 0;
  return `https://dict.youdao.com/dictvoice?type=${type}&audio=${encodeURIComponent(word)}`;
}
