import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { BrowseItem, CardKind, StateFilter } from "../types";

const LETTERS = "abcdefghijklmnopqrstuvwxyz".split("");

const STATES: { key: StateFilter; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "new", label: "未学" },
  { key: "learning", label: "学习中" },
  { key: "review", label: "复习" },
  { key: "mastered", label: "已掌握" },
];

const STATE_LABEL: Record<number, string> = {
  0: "未学",
  1: "学习中",
  2: "复习",
  3: "已掌握",
};

/** 后端分页大小：几万词靠「滚动到底再拉下一页」，而不是一次性全取。 */
const PAGE_SIZE = 100;
/** 行高固定，虚拟滚动才能用 O(1) 算式定位。改样式时记得同步 `.list-item`。 */
const ROW_H = 64;
/** 视口上下各多渲染几行，避免快速滚动露白。 */
const OVERSCAN = 6;

/** 毫秒 → 本地 `YYYY-MM-DD`（不能用 toISOString，那是 UTC，会差一天）。 */
function fmtDate(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

interface Props {
  onToast: (msg: string) => void;
  /** 点某条 → 打开单词/句子详情页 */
  onOpen: (id: string, kind: CardKind) => void;
}

export default function VocabPage({ onToast, onOpen }: Props) {
  const [keyword, setKeyword] = useState("");
  const [state, setState] = useState<StateFilter>("all");
  const [letter, setLetter] = useState<string | null>(null);
  /** 加入日期（本地日历日 `YYYY-MM-DD`，空串 = 不筛） */
  const [addedOn, setAddedOn] = useState("");
  /** false = 按字母升序；true = 按加入时间倒序 */
  const [newestFirst, setNewestFirst] = useState(false);
  const [items, setItems] = useState<BrowseItem[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(600);

  const reqId = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);

  const load = useCallback(
    async (offset: number, append: boolean) => {
      const id = ++reqId.current;
      setLoading(true);
      try {
        const page = await api.browseWords(
          keyword,
          state,
          letter,
          addedOn || null,
          newestFirst,
          offset,
          PAGE_SIZE,
        );
        if (id !== reqId.current) return;
        setItems((prev) => (append ? [...prev, ...page.items] : page.items));
        setTotal(page.total);
        setHasMore(page.has_more);
        if (!append) {
          listRef.current?.scrollTo({ top: 0 });
          setScrollTop(0);
        }
      } catch (e) {
        onToast(`加载词库失败：${e}`);
      } finally {
        if (id === reqId.current) setLoading(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [keyword, state, letter, addedOn, newestFirst],
  );

  // 关键词防抖；筛选条件变化立即刷新
  useEffect(() => {
    const t = window.setTimeout(() => load(0, false), keyword ? 250 : 0);
    return () => window.clearTimeout(t);
  }, [load, keyword]);

  useEffect(() => {
    const el = listRef.current;
    if (el) setViewportH(el.clientHeight || 600);
  }, []);

  // 只渲染视口附近的行：几万词也不会产生几万个 DOM 节点
  const { start, end } = useMemo(() => {
    const s = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
    const e = Math.min(
      items.length,
      Math.ceil((scrollTop + viewportH) / ROW_H) + OVERSCAN,
    );
    return { start: s, end: Math.max(s, e) };
  }, [scrollTop, viewportH, items.length]);

  const onScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    setScrollTop(el.scrollTop);
    setViewportH(el.clientHeight);
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 400 && hasMore && !loading) {
      load(items.length, true);
    }
  };

  return (
    <div className="vpage">
      <div className="h1">词库</div>

      <input
        placeholder="搜索单词（前缀匹配）"
        value={keyword}
        onChange={(e) => {
          setKeyword(e.target.value);
          setLetter(null);
        }}
      />

      {/* 按加入日期搜索 */}
      <div className="daterow">
        <span className="muted">加入日期</span>
        <input
          className="date-input"
          type="date"
          value={addedOn}
          onChange={(e) => setAddedOn(e.target.value)}
        />
        <button
          className="btn ghost small"
          disabled={!addedOn}
          onClick={() => setAddedOn("")}
        >
          清除
        </button>
      </div>

      <div className="chips">
        {STATES.map((s) => (
          <button
            key={s.key}
            className={state === s.key ? "on" : ""}
            onClick={() => setState(s.key)}
          >
            {s.label}
          </button>
        ))}
      </div>

      <div className="letters">
        <button
          className={letter === null ? "on" : ""}
          onClick={() => {
            setLetter(null);
            setKeyword("");
          }}
        >
          全部
        </button>
        {LETTERS.map((ch) => (
          <button
            key={ch}
            className={letter === ch ? "on" : ""}
            onClick={() => {
              setLetter(ch);
              setKeyword("");
            }}
          >
            {ch}
          </button>
        ))}
      </div>

      <div className="row between count-row">
        <span className="muted">
          共 {total} 条{addedOn ? ` · ${addedOn} 加入` : ""}
        </span>
        <button className="link" onClick={() => setNewestFirst((v) => !v)}>
          {newestFirst ? "按加入时间 ↓" : "按字母 A-Z"}
        </button>
      </div>

      <div className="vlist" ref={listRef} onScroll={onScroll}>
        {items.length === 0 && !loading && <div className="empty">没有匹配的单词</div>}
        <div className="vlist-inner" style={{ height: items.length * ROW_H }}>
          {items.slice(start, end).map((it, i) => (
            <div
              className="list-item vrow tappable"
              key={it.id}
              style={{ top: (start + i) * ROW_H }}
              onClick={() => onOpen(it.id, "word")}
            >
              <div className="main">
                <div className="title">{it.text}</div>
                <div className="sub">
                  {it.gloss || "—"}
                  {it.added_at ? ` · ${fmtDate(it.added_at)}` : ""}
                </div>
              </div>
              <span className={`pill s${it.state}`}>{STATE_LABEL[it.state] ?? it.state}</span>
            </div>
          ))}
        </div>
        {loading && (
          <div className="muted" style={{ textAlign: "center", padding: 10 }}>
            <span className="spinner" />
          </div>
        )}
        {!hasMore && items.length > 0 && !loading && (
          <div className="muted" style={{ textAlign: "center", padding: 10 }}>
            已经到底了
          </div>
        )}
      </div>
    </div>
  );
}
