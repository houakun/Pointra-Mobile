import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import WordSheet from "../components/WordSheet";
import ArticlePicks from "../components/ArticlePicks";
import { useBackGuard } from "../hooks/useBackGuard";
import type {
  Accent,
  ArticleDetail,
  ArticleSummary,
  WordLookup,
} from "../types";

/** 分类筛选项：先按考试分，再按年份分组 */
type ArticleFilter = "all" | "cet4" | "cet6" | "other";

const FILTERS: { key: ArticleFilter; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "cet4", label: "四级" },
  { key: "cet6", label: "六级" },
  { key: "other", label: "示例 / 导入" },
];

interface Props {
  accent: Accent;
  onToast: (msg: string) => void;
  /** 收藏单词等会改变统计的操作，通知外层刷新（今日/词库数字） */
  onChanged: () => void;
  /** 是否正当前台展示：切到别的 tab 时不要拦 Android 返回键（页面并没在展示） */
  active: boolean;
}

/**
 * 选中浮条的高度估算（单行按钮 + 上下 padding），
 * 用来判断选区旁边还放不放得下。
 */
const PICK_BAR_H = 46;

/** 浮条的落点：选区下方 / 选区上方 / 停到底部导航上方（跨屏大选区时兼用） */
interface PickPlacement {
  place: "below" | "above" | "dock";
  top?: number;
  bottom?: number;
}

/**
 * 算浮条落在哪。
 *
 * 优先放在**选区下方**：Android 的系统「复制 / 分享 / 全选」菜单默认贴在选区上方，
 * 放下面两边互不遮挡（之前固定在屏幕最底部，离手指和选区都太远）。
 * 下方不够就翻到选区上方（向上展开）；连上方都放不下（整段跨屏全选）就停到底部导航上方。
 */
function pickPlacement(rect: DOMRect): PickPlacement {
  const vh = window.innerHeight;
  const gap = 8;
  if (rect.bottom + gap + PICK_BAR_H < vh - 84) {
    return { place: "below", top: rect.bottom + gap };
  }
  // 用「距离视口底部多少」做锤点，向上展开时高度才自适应
  const aboveBottom = vh - rect.top + gap;
  if (aboveBottom + PICK_BAR_H < vh - 64) {
    return { place: "above", bottom: aboveBottom };
  }
  return { place: "dock" };
}

interface Token {
  text: string;
  /** `true` = 可点击的英文单词 */
  word: boolean;
  /** 该词所在句子（词卡「文中」用） */
  sentence?: string;
}

/**
 * 把段落切成「单词 / 非单词」token。
 *
 * 词的定义包含内部连字符与撇号（`sugar-free`、`company's` 各算一个词），
 * 否则点 `sugar-free` 只会查到 `sugar`。
 */
function tokenize(paragraph: string): Token[] {
  const tokens: Token[] = [];
  const re = /[A-Za-z]+(?:['’-][A-Za-z]+)*/g;
  let last = 0;
  for (const match of paragraph.matchAll(re)) {
    const start = match.index ?? 0;
    if (start > last) {
      tokens.push({ text: paragraph.slice(last, start), word: false });
    }
    tokens.push({
      text: match[0],
      word: true,
      sentence: sentenceAt(paragraph, start),
    });
    last = start + match[0].length;
  }
  if (last < paragraph.length) {
    tokens.push({ text: paragraph.slice(last), word: false });
  }
  return tokens;
}

/** 找出包含 `index` 处的句子（含句末标点），用于词卡的「文中译文」。 */
function sentenceAt(text: string, index: number): string {
  const re = /[.!?]+[\s"')\]]*/g;
  let start = 0;
  let end = text.length;
  for (const match of text.matchAll(re)) {
    const boundaryEnd = (match.index ?? 0) + match[0].length;
    if (boundaryEnd <= index) {
      start = boundaryEnd;
      continue;
    }
    end = boundaryEnd;
    break;
  }
  const sentence = text.slice(start, end).trim();
  return sentence || text.trim();
}

/**
 * 把黏贴的多篇文本切成若干篇。
 *
 * 分隔符：**独占一行的 `---`**（3 个以上连字符）。
 * 每篇首行若以 `#` 开头，就当作该篇标题（`# ` 会被去掉）。
 * 只有一篇时返回 1 项，调用方可以用输入框里的标题覆盖它。
 */
export function splitBatch(raw: string): { title: string; body: string }[] {
  return raw
    .split(/^\s*-{3,}\s*$/m)
    .map((block) => block.trim())
    .filter((block) => block.length > 0)
    .map((block) => {
      const lines = block.split("\n");
      const first = (lines[0] ?? "").trim();
      if (first.startsWith("#")) {
        return {
          title: first.replace(/^#+\s*/, "").trim(),
          body: lines.slice(1).join("\n").trim(),
        };
      }
      return { title: "", body: block };
    });
}

/**
 * 文章所属的「本地年月」（`YYYY-MM`）。
 *
 * 真题用 id 里解析出的年月（`cet6-2025-06-1` → `2025-06`）；
 * 自写示例与导入文章没有年月字段，就用加入时间的**本地**年月（跟用户看到的一致）。
 */
function monthKeyOf(a: ArticleSummary): string {
  if (a.year && a.month) return `${a.year}-${String(a.month).padStart(2, "0")}`;
  const d = new Date(a.created_at);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

/** 卡片标题：`2026 年 · 3 月` */
function monthLabel(key: string): string {
  const [year, month] = key.split("-");
  return `${year} 年 · ${Number(month)} 月`;
}

export default function ReaderPage({ accent, onToast, onChanged, active }: Props) {
  const [articles, setArticles] = useState<ArticleSummary[]>([]);
  const [listLoading, setListLoading] = useState(true);

  const [article, setArticle] = useState<ArticleDetail | null>(null);
  /** 段落下标 → 译文；已取到的译文收起后再展开不会重新请求 */
  const [translations, setTranslations] = useState<Record<number, string>>({});
  /** 段落下标 → 是否展开显示译文 */
  const [shown, setShown] = useState<Record<number, boolean>>({});
  const [translating, setTranslating] = useState<number | null>(null);
  /** 文章标题的译文（点标题旁的「译」才请求） */
  const [titleTrans, setTitleTrans] = useState<string | null>(null);
  const [titleTranslating, setTitleTranslating] = useState(false);

  // --- 选中句子后的操作条（翻译 / 加入句子本）---
  /** 当前选中的文本（空 = 没选中） */
  const [picked, setPicked] = useState("");
  /** 浮条落点；选中文本很长时也要算出个能用的位置 */
  const [pickPlace, setPickPlace] = useState<PickPlacement | null>(null);
  const [pickTrans, setPickTrans] = useState<string | null>(null);
  const [pickBusy, setPickBusy] = useState<"translate" | "save" | null>(null);

  // 词卡
  const [sheetOpen, setSheetOpen] = useState(false);
  /** 当前打开的月份（`YYYY-MM`）；null = 还停在「年 · 月」卡片网格首页 */
  const [month, setMonth] = useState<string | null>(null);
  /** 正在打开的文章 id（文章大/网络慢时先给个反馈，而不是几秒没动静） */
  const [openingId, setOpeningId] = useState<string | null>(null);
  /** 打开文章的请求序号：返回时作废迟到的响应 */
  const articleReqId = useRef(0);
  /** 本篇速记（这篇文章收藏的生词 / 生句）是否打开 */
  const [picksOpen, setPicksOpen] = useState(false);
  const [sheetLoading, setSheetLoading] = useState(false);
  const [sheetError, setSheetError] = useState<string | null>(null);
  const [lookup, setLookup] = useState<WordLookup | null>(null);
  /** 连点两个词时，只认最后一次请求的结果 */
  const lookupSeq = useRef(0);

  // 导入
  const [importing, setImporting] = useState(false);
  const [importTitle, setImportTitle] = useState("");
  const [importBody, setImportBody] = useState("");
  const [saving, setSaving] = useState(false);

  const loadList = useCallback(async () => {
    setListLoading(true);
    try {
      setArticles(await api.listArticles());
    } catch (e) {
      onToast(`读取文章列表失败：${e}`);
    } finally {
      setListLoading(false);
    }
  }, [onToast]);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  /** 滚动容器（App 里那个 .app-body）；列表与文章共用它 */
  const scroller = () => document.querySelector<HTMLElement>(".app-body");
  /** 进文章前记下的列表滚动位置 */
  const listScroll = useRef(0);
  /** 待还原的列表位置（null = 不需要还原） */
  const pendingRestore = useRef<number | null>(null);

  const closeArticle = useCallback(() => {
    // 返回列表时回到「我刚才点的那篇」所在的位置，而不是正文里的那个偏移量
    articleReqId.current += 1; // 作废仍在路上的打开请求
    setOpeningId(null);
    pendingRestore.current = listScroll.current;
    setArticle(null);
  }, []);

  /** 进入某个月份：记住网格位置，新页从顶部开始读 */
  const openMonth = useCallback((key: string) => {
    gridScroll.current = scroller()?.scrollTop ?? 0;
    setMonth(key);
    requestAnimationFrame(() => scroller()?.scrollTo({ top: 0 }));
  }, []);

  /** 从月份回到网格：还原网格位置 */
  const closeMonth = useCallback(() => {
    articleReqId.current += 1; // 作废仍在路上的打开请求
    setOpeningId(null);
    setMonth(null);
    // 双 rAF：等网格重新渲染完再设 scrollTop，否则会被钳成 0
    requestAnimationFrame(() =>
      requestAnimationFrame(() => scroller()?.scrollTo({ top: gridScroll.current })),
    );
  }, []);

  const clearPick = useCallback(() => {
    window.getSelection()?.removeAllRanges();
    setPicked("");
    setPickPlace(null);
  }, []);

  // 返回键优先级：本篇速记 → 词卡 → 取消选中 → 文章正文 → 月份列表 → 年月网格。
  // 必须带上 `active`：切到别的 tab 时本页只是被 display 隐藏，不该再拦返回键。
  useBackGuard(
    (article !== null || month !== null || sheetOpen || picksOpen) && active,
    useCallback(() => {
      if (picksOpen) {
        setPicksOpen(false);
        return;
      }
      if (sheetOpen) {
        setSheetOpen(false);
        return;
      }
      if (picked) {
        clearPick();
        return;
      }
      if (article !== null) {
        closeArticle();
        return;
      }
      if (month !== null) {
        closeMonth();
      }
      // 【依赖必须列全】少了 `article` 就会抱到旧闭包：
      // 刚打开文章时回调里读到的还是 article=null，于是“返回一次”去关月份列表
      // （表面上就是返回无效），再按一次又因为 month 已变而跳层。踩过。
    }, [picksOpen, sheetOpen, picked, article, clearPick, closeArticle, month, closeMonth]),
  );

  const openArticle = useCallback(
    async (id: string) => {
      // 先记位置再切：切完列表 DOM 就没了，读不到 scrollTop
      listScroll.current = scroller()?.scrollTop ?? 0;
      pendingRestore.current = null;
      const seq = ++articleReqId.current;
      setOpeningId(id);
      try {
        const detail = await api.getArticle(id);
        // 【关键】请求回来时用户可能已经退回去了（文章大 / 网络慢时要好几秒）：
        // 不判这一下的话，退到了月份或网格后会被迟到的事件重新推回正文，
        // 表现就是“返回了一次却没反应，再按一次又跳过一层”。
        if (seq !== articleReqId.current) return;
        setArticle(detail);
        setTranslations({});
        setShown({});
        setTranslating(null);
        setTitleTrans(null);
        setTitleTranslating(false);
        setSheetOpen(false);
        setLookup(null);
        setPicksOpen(false);
        // 打开的是一篇新文章，从头读（不沿用列表那个偏移量，否则会停在正文中间）
        requestAnimationFrame(() => scroller()?.scrollTo({ top: 0 }));
      } catch (e) {
        if (seq === articleReqId.current) onToast(`打开文章失败：${e}`);
      } finally {
        if (seq === articleReqId.current) setOpeningId(null);
      }
    },
    [onToast],
  );

  // 回到列表后还原滚动位置。
  // 双 rAF：等列表真正渲染出来再设 scrollTop，否则会被当成「超出内容高度」钳成 0。
  useEffect(() => {
    if (article !== null) return;
    const top = pendingRestore.current;
    if (top === null) return;
    pendingRestore.current = null;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => scroller()?.scrollTo({ top })),
    );
  }, [article]);

  const openWord = useCallback(
    // 只传单词：不再为每个词请求它在文中的那句话译文
    // （词卡里那块「文中」已经按要求去掉了，少一次网络往返）
    async (word: string) => {
      const seq = ++lookupSeq.current;
      setSheetOpen(true);
      setSheetLoading(true);
      setSheetError(null);
      setLookup(null);
      try {
        const result = await api.lookupWord(word, null);
        if (seq !== lookupSeq.current) return;
        setLookup(result);
      } catch (e) {
        if (seq !== lookupSeq.current) return;
        setSheetError(String(e));
      } finally {
        if (seq === lookupSeq.current) setSheetLoading(false);
      }
    },
    [],
  );

  const toggleTranslation = useCallback(
    async (index: number, text: string) => {
      // 已取到 → 只切显隐，不再请求（成功标准 3 的「不重复请求」）
      if (translations[index] !== undefined) {
        setShown((prev) => ({ ...prev, [index]: !prev[index] }));
        return;
      }
      setTranslating(index);
      try {
        const result = await api.translateParagraph(text);
        setTranslations((prev) => ({ ...prev, [index]: result.trans }));
        setShown((prev) => ({ ...prev, [index]: true }));
      } catch (e) {
        onToast(String(e));
      } finally {
        setTranslating(null);
      }
    },
    [translations, onToast],
  );

  const removeArticle = useCallback(
    async (id: string, title: string) => {
      // 约束：单条删除必须二次确认
      if (!window.confirm(`删除《${title}》？删除后不可恢复。`)) return;
      try {
        await api.deleteArticle(id);
        onToast("已删除");
        await loadList();
      } catch (e) {
        onToast(`删除失败：${e}`);
      }
    },
    [loadList, onToast],
  );

  const saveImport = useCallback(async () => {
    const batches = splitBatch(importBody);
    if (batches.length === 0) {
      onToast("正文不能为空");
      return;
    }
    setSaving(true);
    try {
      let saved = 0;
      let firstId = "";
      let firstName = "";
      let firstParagraphs = 0;
      const errors: string[] = [];

      for (const item of batches) {
        // 单篇：用输入框里的标题；多篇：用每篇自己的「# 标题」行
        const title = batches.length === 1 ? importTitle : item.title;
        try {
          const created = await api.saveArticle(title, item.body);
          if (!firstId) {
            firstId = created.id;
            firstName = created.title;
            firstParagraphs = created.paragraphs.length;
          }
          saved += 1;
        } catch (e) {
          errors.push(String(e));
        }
      }

      if (saved === 0) {
        onToast(`导入失败：${errors[0] ?? "未知错误"}`);
        return;
      }

      setImporting(false);
      setImportTitle("");
      setImportBody("");
      await loadList();
      onToast(
        saved === 1
          ? `已导入《${firstName}》，共 ${firstParagraphs} 段`
          : `已导入 ${saved} 篇${errors.length > 0 ? `（有 ${errors.length} 篇失败）` : ""}`,
      );
      await openArticle(firstId);
    } finally {
      setSaving(false);
    }
  }, [importTitle, importBody, loadList, onToast, openArticle]);

  // 分类：考试筛选 + 年份分组（素材量大了之后，平铺 140+ 篇根本找不着）
  const [filter, setFilter] = useState<ArticleFilter>("all");
  /** 卡片网格的滚动位置：从某个月返回时还原 */
  const gridScroll = useRef(0);

  const counts = useMemo(() => {
    const c = { all: articles.length, cet4: 0, cet6: 0, other: 0 };
    for (const a of articles) {
      if (a.exam === "cet4") c.cet4 += 1;
      else if (a.exam === "cet6") c.cet6 += 1;
      else c.other += 1;
    }
    return c;
  }, [articles]);

  const visible = useMemo(
    () =>
      articles.filter((a) =>
        filter === "all" ? true : filter === "other" ? !a.exam : a.exam === filter,
      ),
    [articles, filter],
  );

  /** 按「年 · 月」聚合：网格首页的每张卡片就是一个月的文章 */
  const months = useMemo(() => {
    const map = new Map<string, ArticleSummary[]>();
    for (const item of visible) {
      const key = monthKeyOf(item);
      const list = map.get(key);
      if (list) list.push(item);
      else map.set(key, [item]);
    }
    // 「年 - 月」倒序（新的在前）
    return [...map.entries()].sort(([a], [b]) => b.localeCompare(a));
  }, [visible]);

  /** 当前月份里的文章；用同一份 `visible`，所以 chips 筛选状态不会串 */
  const monthArticles = useMemo(
    () => (month === null ? [] : visible.filter((a) => monthKeyOf(a) === month)),
    [month, visible],
  );

  /**
   * 标题旁的「译」：只翻标题。
   *
   * 不进段落译文缓存（标题很短、也不会重复出现），收起只是把本地状态置空。
   */
  const toggleTitleTranslation = useCallback(async () => {
    if (!article) return;
    if (titleTrans) {
      setTitleTrans(null);
      return;
    }
    setTitleTranslating(true);
    try {
      const result = await api.translateParagraph(article.title);
      setTitleTrans(result.trans);
    } catch (e) {
      onToast(String(e));
    } finally {
      setTitleTranslating(false);
    }
  }, [article, titleTrans, onToast]);

  /**
   * 监听 WebView 的文本选择：选中就弹操作条。
   *
   * 只读取选区、**不拦系统菜单** —— 「复制 / 分享 / 网页搜索」继续可用。
   * 只过滤没空格的（单个单词，交给点词查词）；**不限长度**：
   * 原先卡在 300 字，长句反而弹不出「翻译 / 句子本」，跟功能目的相反。
   */
  useEffect(() => {
    if (!article) {
      setPicked("");
      setPickPlace(null);
      return;
    }
    const sync = () => {
      const sel = window.getSelection();
      const text = sel ? sel.toString().trim() : "";
      if (!sel || sel.rangeCount === 0 || !text || !text.includes(" ")) {
        setPicked("");
        setPickPlace(null);
        return;
      }
      const rect = sel.getRangeAt(0).getBoundingClientRect();
      setPicked((prev) => (prev === text ? prev : text));
      setPickPlace(pickPlacement(rect));
    };
    document.addEventListener("selectionchange", sync);
    // 滚动/转屏后选区在屏幕上会移动，浮条得跟着，否则会飘在原来的地方
    const body = document.querySelector(".app-body");
    body?.addEventListener("scroll", sync, { passive: true });
    window.addEventListener("resize", sync);
    return () => {
      document.removeEventListener("selectionchange", sync);
      body?.removeEventListener("scroll", sync);
      window.removeEventListener("resize", sync);
    };
  }, [article]);

  // 选区一变，上一次的译文就作废
  useEffect(() => {
    setPickTrans(null);
  }, [picked]);

  const translatePick = useCallback(async () => {
    if (!picked) return;
    setPickBusy("translate");
    try {
      const result = await api.translateParagraph(picked);
      setPickTrans(result.trans);
    } catch (e) {
      onToast(String(e));
    } finally {
      setPickBusy(null);
    }
  }, [picked, onToast]);

  const savePick = useCallback(async () => {
    if (!picked) return;
    setPickBusy("save");
    try {
      // 还没翻过就顺手翻一下再存：句子本里直接带译文更实用（翻译失败也允许只存原文）
      let trans = pickTrans;
      if (!trans) {
        try {
          trans = (await api.translateParagraph(picked)).trans;
          setPickTrans(trans);
        } catch {
          /* 忽略：只存原文 */
        }
      }
      const result = await api.addLocalSentence(picked, trans ?? null, article?.id ?? null);
      onToast(result.existed ? "这句已在句子本里，已刷新译文" : "已加入句子本");
      onChanged();
      clearPick();
    } catch (e) {
      onToast(`加入失败：${e}`);
    } finally {
      setPickBusy(null);
    }
  }, [picked, pickTrans, article, onToast, onChanged, clearPick]);

  // 按钮文字写成函数而不是嵌套三元：oxlint 的 no-nested-ternary / readability 都把嵌套三元当问题
  function titleButtonLabel(): string {
    if (titleTranslating) return "…";
    if (titleTrans) return "收起";
    return "译";
  }

  function pickTranslateLabel(): string {
    if (pickBusy === "translate") return "翻译中…";
    if (pickTrans) return "重新翻译";
    return "翻译";
  }

  const keywords = useMemo(
    () => new Set((article?.keywords ?? []).map((k) => k.toLowerCase())),
    [article],
  );

  // --- 阅读视图 ------------------------------------------------------------
  if (article) {
    const paragraphs = article.paragraphs;
    return (
      <div className="reader">
        <div className="reader-head">
          <button className="btn ghost small" onClick={closeArticle}>
            ← 文章
          </button>
          <span className="muted reader-tip">点词查词 · 点「译」出译文</span>
          {/* 右上角「速记」：只看这篇收藏的生词与生句 */}
          <button className="btn ghost small" onClick={() => setPicksOpen(true)}>
            速记
          </button>
        </div>

        <div className="reader-title-row">
          <div className="h1 reader-title">{article.title}</div>
          <button
            className="para-btn"
            disabled={titleTranslating}
            onClick={() => void toggleTitleTranslation()}
          >
            {titleButtonLabel()}
          </button>
        </div>
        {titleTrans && <div className="para-trans title-trans">{titleTrans}</div>}
        {article.origin && (
          <div className="muted reader-origin">{article.origin}</div>
        )}
        {keywords.size > 0 && (
          <div className="muted reader-hint">
            带下划线的词是四六级核心词（共 {keywords.size} 个）
          </div>
        )}

        <div className="reader-body">
          {paragraphs.map((text, index) => (
            <ParagraphBlock
              key={index}
              text={text}
              keywords={keywords}
              translation={translations[index]}
              shown={Boolean(shown[index])}
              translating={translating === index}
              onToggle={() => void toggleTranslation(index, text)}
              onWord={(word) => void openWord(word)}
            />
          ))}
        </div>

        {/* 选中后的操作条：浮在选区旁（下方优先），与系统「复制/分享」菜单共存 */}
        {picked && pickPlace && (
          <div
            className={`pick-bar ${pickPlace.place}${pickTrans ? " open" : ""}`}
            style={
              pickPlace.place === "below"
                ? ({
                    top: pickPlace.top,
                    // 给 CSS 算 max-height 用（展开译文时不要超出屏幕）
                    "--pick-top": `${pickPlace.top}px`,
                  } as React.CSSProperties)
                : pickPlace.place === "above"
                  ? ({
                      bottom: pickPlace.bottom,
                      "--pick-bottom": `${pickPlace.bottom}px`,
                    } as React.CSSProperties)
                  : undefined
            }
          >
            <div className="pick-row">
              {/* onPointerDown 拦一下：否则点按钮会先清掉选区，浮条自己就消失了 */}
              <button
                className="btn ghost small"
                disabled={pickBusy !== null}
                onPointerDown={(e) => e.preventDefault()}
                onClick={() => void translatePick()}
              >
                {pickTranslateLabel()}
              </button>
              <button
                className="btn small"
                disabled={pickBusy !== null}
                onPointerDown={(e) => e.preventDefault()}
                onClick={() => void savePick()}
              >
                {pickBusy === "save" ? <span className="spinner" /> : "＋ 句子本"}
              </button>
              <span className="pick-len muted">{picked.length} 字</span>
              <button
                className="btn ghost small"
                onPointerDown={(e) => e.preventDefault()}
                onClick={clearPick}
                aria-label="取消选择"
              >
                ✕
              </button>
            </div>
            {pickTrans && (
              <div className="pick-body">
                <div className="pick-text">{picked}</div>
                <div className="pick-trans">{pickTrans}</div>
              </div>
            )}
          </div>
        )}

        {sheetOpen && (
          <WordSheet
            loading={sheetLoading}
            error={sheetError}
            lookup={lookup}
            accent={accent}
            articleId={article.id}
            onClose={() => setSheetOpen(false)}
            onToast={onToast}
            onAdded={onChanged}
          />
        )}

        {picksOpen && (
          <ArticlePicks
            articleId={article.id}
            articleTitle={article.title}
            onClose={() => setPicksOpen(false)}
            onToast={onToast}
          />
        )}
      </div>
    );
  }

  // --- 某个月份的文章列表 --------------------------------------------------
  if (month !== null) {
    return (
      <div>
        <div className="reader-head">
          <button className="btn ghost small" onClick={closeMonth}>
            ← 月份
          </button>
          <div className="h1 month-head-title">{monthLabel(month)}</div>
          <span className="muted">{monthArticles.length} 篇</span>
        </div>

        {!listLoading && monthArticles.length === 0 && (
          <div className="empty">这个月没有文章了</div>
        )}

        <div className="card" style={{ padding: "4px 14px" }}>
          {monthArticles.map((item) => (
            <div className="list-item" key={item.id}>
              <div className="main tappable" onClick={() => void openArticle(item.id)}>
                <div className="title" style={{ whiteSpace: "normal" }}>
                  {item.title}
                </div>
                <div className="sub origin">
                  {openingId === item.id
                    ? "正在打开…"
                    : `${item.origin || "我的导入"} · ${item.paragraph_count} 段`}
                </div>
              </div>
              <button
                className="btn danger small"
                onClick={() => void removeArticle(item.id, item.title)}
              >
                删除
              </button>
            </div>
          ))}
        </div>
      </div>
    );
  }

  // --- 阅读页首页：按「年 · 月」的卡片网格 ----------------------------------
  return (
    <div>
      <div className="reader-head">
        <div className="h1">阅读</div>
        <button className="btn ghost small" onClick={() => setImporting(true)}>
          ＋ 导入
        </button>
      </div>

      {listLoading && (
        <div className="empty">
          <span className="spinner" />
        </div>
      )}

      {!listLoading && articles.length === 0 && (
        <div className="empty">还没有文章，点右上角「导入」粘贴一篇</div>
      )}

      {/* 分类筛选：计数放在标签里，一眼看出各级/各年有多少篇 */}
      {articles.length > 0 && (
        <div className="chips reader-filters">
          {FILTERS.map((f) => (
            <button
              key={f.key}
              className={filter === f.key ? "on" : ""}
              onClick={() => setFilter(f.key)}
            >
              {f.label} {counts[f.key]}
            </button>
          ))}
        </div>
      )}

      {!listLoading && visible.length === 0 && (
        <div className="empty">这个分类下还没有文章</div>
      )}

      <div className="month-grid">
        {months.map(([key, items]) => (
          <button className="month-card" key={key} onClick={() => openMonth(key)}>
            <span className="month-icon" aria-hidden="true">
              {/* 日历图标：内联 SVG，跟着主题色走，不引外部图标库 */}
              <svg
                viewBox="0 0 24 24"
                width="20"
                height="20"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <rect x="3" y="4.5" width="18" height="16" rx="3" />
                <path d="M3 9.5h18M8 3v3M16 3v3" />
              </svg>
            </span>
            <span className="month-title">{monthLabel(key)}</span>
            <span className="month-foot">
              <span className="month-count">{items.length} 篇文章</span>
              <span className="month-arrow">›</span>
            </span>
          </button>
        ))}
      </div>

      {importing && (
        <div className="sheet-mask" onClick={() => setImporting(false)}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <div className="sheet-grip" />
            <div className="sheet-head">
              <div className="sheet-word">导入文章</div>
              <button className="btn ghost small" onClick={() => setImporting(false)}>
                ✕
              </button>
            </div>
            <div className="sheet-body">
              <div className="field">
                <label>标题（可留空）</label>
                <input
                  value={importTitle}
                  onChange={(e) => setImportTitle(e.target.value)}
                  placeholder="例如：2023 年 6 月真题 Passage 1"
                />
              </div>
              <div className="field">
                <label>正文</label>
                <textarea
                  className="textarea"
                  rows={10}
                  value={importBody}
                  onChange={(e) => setImportBody(e.target.value)}
                  placeholder="粘贴英文文章…（空行分段；多篇用独占一行的 --- 分隔，每篇首行可写 # 标题）"
                />
              </div>
              <button className="btn primary wide" disabled={saving} onClick={() => void saveImport()}>
                {saving ? "保存中…" : "保存"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function ParagraphBlock({
  text,
  keywords,
  translation,
  shown,
  translating,
  onToggle,
  onWord,
}: {
  text: string;
  keywords: Set<string>;
  translation: string | undefined;
  shown: boolean;
  translating: boolean;
  onToggle: () => void;
  onWord: (word: string) => void;
}) {
  const tokens = useMemo(() => tokenize(text), [text]);

  return (
    <div className="para">
      <div className="para-text">
        {tokens.map((token, i) =>
          token.word ? (
            <span
              key={i}
              className={keywords.has(token.text.toLowerCase()) ? "rw kw" : "rw"}
              onClick={() => onWord(token.text)}
            >
              {token.text}
            </span>
          ) : (
            <span key={i}>{token.text}</span>
          ),
        )}
      </div>

      <button className="para-btn" onClick={onToggle} disabled={translating}>
        {translating ? "…" : shown ? "收起" : "译"}
      </button>

      {shown && translation !== undefined && (
        <div className="para-trans">{translation}</div>
      )}
    </div>
  );
}
