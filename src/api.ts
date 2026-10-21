import { invoke } from "@tauri-apps/api/core";
import type {
  ArticleDetail,
  ArticlePicks,
  ArticleSummary,
  BrowsePage,
  CardDetail,
  CardKind,
  LocalSentenceAdd,
  LocalWordAdd,
  Overview,
  ParagraphTranslation,
  PingInfo,
  Progress,
  QueueEntry,
  ServerHit,
  Settings,
  StateFilter,
  SyncReport,
  UpdateInfo,
  WordLookup,
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
  /** 扫描同网段 31415，找出正在开「手机同步」的电脑（电脑 IP 变了也能找回） */
  scanServers: (port?: number | null) =>
    invoke<ServerHit[]>("scan_servers", { port: port ?? null }),
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
  /** 句子本：按「本地自然日」看一天（`addedOn` = `YYYY-MM-DD`），可叠加关键词 */
  browseSentences: (
    keyword: string,
    addedOn: string | null,
    offset: number,
    limit: number,
  ) =>
    invoke<BrowsePage>("browse_sentences", {
      keyword,
      addedOn,
      utcOffsetMinutes: tzOffset(),
      offset,
      limit,
    }),
  /** 最近一次收藏句子的日期（句子本打开时定位到这天；一条都没有时为 null） */
  latestSentenceDay: () =>
    invoke<string | null>("latest_sentence_day", { utcOffsetMinutes: tzOffset() }),
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

  // --- 阅读 ---------------------------------------------------------------

  listArticles: () => invoke<ArticleSummary[]>("list_articles"),
  getArticle: (id: string) => invoke<ArticleDetail>("get_article", { id }),
  saveArticle: (title: string, body: string) =>
    invoke<ArticleDetail>("save_article", { title, body }),
  /** 删除单篇文章（UI 层负责二次确认） */
  deleteArticle: (id: string) => invoke<void>("delete_article", { id }),

  /** 查词：词典数据 + 该词所在句子的译文；结果在 Rust 侧缓存 */
  lookupWord: (word: string, sentence: string | null) =>
    invoke<WordLookup>("lookup_word", { word, sentence }),
  /** 翻译整段（段落「译」按钮）；结果在 Rust 侧缓存 */
  translateParagraph: (text: string) =>
    invoke<ParagraphTranslation>("translate_paragraph", { text }),
  /** 加入本机单词本（本机 id/seq 与 PC 下发的隔开，见 Rust 侧注释）
   *  `articleId` = 从阅读页收藏时带上的文章 id，用来记「本篇生词」。 */
  addLocalWord: (word: string, articleId?: string | null) =>
    invoke<LocalWordAdd>("add_local_word", { word, articleId: articleId ?? null }),

  /** 当前 App 版本（Rust 侧读编译进二进制的包版本，不走 ACL） */
  appVersion: () => invoke<string>("app_version"),

  /** 把阅读页选中的句子加进本机句子本（译文可选，不依赖电脑端） */
  addLocalSentence: (text: string, trans: string | null, articleId?: string | null) =>
    invoke<LocalSentenceAdd>("add_local_sentence", {
      text,
      trans,
      articleId: articleId ?? null,
    }),

  /** 某篇文章里收藏过的生词与生句（阅读页右上角「速记」） */
  listArticlePicks: (articleId: string) =>
    invoke<ArticlePicks>("list_article_picks", { articleId }),
};

/** 有道在线发音 URL（R3）。Android 端只用 HTML5 audio 播放，不移植 rodio。 */
export function youdaoAudio(word: string, accent: "us" | "uk"): string {
  const type = accent === "uk" ? 1 : 0;
  return `https://dict.youdao.com/dictvoice?type=${type}&audio=${encodeURIComponent(word)}`;
}
