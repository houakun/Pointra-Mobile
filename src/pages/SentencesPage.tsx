import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { BrowseItem } from "../types";

const PAGE_SIZE = 40;

/** 毫秒 → 本地 `YYYY-MM-DD`（不能用 toISOString，那是 UTC，会差一天）。 */
function dayKey(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 今天（本地自然日）。 */
function today(): string {
  return dayKey(Date.now());
}

/**
 * 在 `YYYY-MM-DD` 上加减天数。
 *
 * 用 `setDate` 而不是加减 86400000：夏令时切换那天会差一小时，跨月跨年也交给 Date 处理。
 */
function shiftDay(day: string, delta: number): string {
  const [y, m, d] = day.split("-").map(Number);
  const dt = new Date(y, m - 1, d);
  dt.setDate(dt.getDate() + delta);
  return dayKey(dt.getTime());
}

interface Props {
  onToast: (msg: string) => void;
}

/**
 * 句子本：一次看一天（`‹ 2026/10/18 ›`）。
 *
 * - 点 `‹` / `›` 切前一天 / 后一天，点中间的日期直接选某一天（系统日期选择器）
 * - 关键词在当前这一天里筛（后端把「日期区间 + 关键词」一起查）
 * - 打开时定位到「最近有句子的那天」，今天没收藏也不会看到一片空白
 */
export default function SentencesPage({ onToast }: Props) {
  const [keyword, setKeyword] = useState("");
  const [day, setDay] = useState(today);
  const [items, setItems] = useState<BrowseItem[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  /** 展开译文的句子 id（一次只开一条，否则加载多了列表会被撑得很长） */
  const [openId, setOpenId] = useState<string | null>(null);
  const reqId = useRef(0);

  // 打开就先定位到最近有句子的那天（失败就用今天，不打扰用户）
  useEffect(() => {
    let alive = true;
    api
      .latestSentenceDay()
      .then((latest) => {
        if (alive && latest) setDay(latest);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const load = useCallback(
    async (kw: string, on: string, offset: number, append: boolean) => {
      const id = ++reqId.current;
      setLoading(true);
      try {
        const page = await api.browseSentences(kw, on, offset, PAGE_SIZE);
        if (id !== reqId.current) return;
        setItems((prev) => (append ? [...prev, ...page.items] : page.items));
        setTotal(page.total);
        setHasMore(page.has_more);
      } catch (e) {
        onToast(`加载句子本失败：${e}`);
      } finally {
        if (id === reqId.current) setLoading(false);
      }
    },
    [onToast],
  );

  useEffect(() => {
    // 输入关键词时防抖，换日期时立刻查
    const t = window.setTimeout(() => load(keyword, day, 0, false), keyword ? 250 : 0);
    return () => window.clearTimeout(t);
  }, [keyword, day, load]);

  const remove = async (id: string) => {
    try {
      await api.deleteSentence(id);
      setItems((prev) => prev.filter((i) => i.id !== id));
      setTotal((t) => Math.max(0, t - 1));
      onToast("已从句子本删除");
    } catch (e) {
      onToast(`删除失败：${e}`);
    }
  };

  const isToday = day >= today();
  const dateRef = useRef<HTMLInputElement>(null);

  /**
   * 点日期文字 → 弹系统日期选择器。
   *
   * 优先 `showPicker()`（Chrome 99+）：这样外层可以自己渲染成 `2026/10/18`，
   * 不受系统 locale（`10/17/2026`）影响；拿不到就退回直接点原生 input。
   */
  const openPicker = () => {
    const el = dateRef.current;
    if (!el) return;
    const picker = el as HTMLInputElement & { showPicker?: () => void };
    if (typeof picker.showPicker === "function") {
      try {
        picker.showPicker();
        return;
      } catch {
        /* 浏览器不让程序弹选择器时退回 click */
      }
    }
    el.click();
  };

  return (
    <div>
      <div className="h1">句子本</div>

      <input
        placeholder="搜索句子或译文"
        value={keyword}
        onChange={(e) => setKeyword(e.target.value)}
      />

      <div className="day-nav">
        <button
          className="day-arrow"
          onClick={() => setDay(shiftDay(day, -1))}
          aria-label="前一天"
        >
          ‹
        </button>
        {/* 中间是可点的日期：点一下弹系统日期选择器，直接跳到某天 */}
        <button type="button" className="day-pick" onClick={openPicker} aria-label="选择日期">
          {day.replace(/-/g, "/")}
        </button>
        <input
          ref={dateRef}
          className="day-native"
          type="date"
          value={day}
          max={today()}
          onChange={(e) => {
            if (e.target.value) setDay(e.target.value);
          }}
          tabIndex={-1}
          aria-hidden="true"
        />
        <button
          className="day-arrow"
          disabled={isToday}
          onClick={() => setDay(shiftDay(day, 1))}
          aria-label="后一天"
        >
          ›
        </button>
      </div>

      <div className="muted" style={{ margin: "4px 2px 6px" }}>
        共 {total} 条
      </div>

      <div className="card" style={{ padding: "4px 14px" }}>
        {items.length === 0 && !loading && (
          <div className="empty">
            {keyword
              ? "这一天没有匹配的句子"
              : "这一天还没有收藏的句子，用 ‹ › 换一天看看"}
          </div>
        )}
        {items.map((it) => (
          <div className="list-item" key={it.id}>
            <div
              className="main tappable"
              onClick={() => setOpenId(openId === it.id ? null : it.id)}
            >
              <div className="title" style={{ whiteSpace: "normal" }}>
                {it.text}
              </div>
              {/* 译文就地展开，不再跳到一个新页面 */}
              {openId === it.id && (
                <div className="sub" style={{ whiteSpace: "normal" }}>
                  {it.gloss || "（暂无译文）"}
                </div>
              )}
            </div>
            <button className="btn danger small" onClick={() => remove(it.id)}>
              删除
            </button>
          </div>
        ))}
      </div>

      {hasMore && (
        <button
          className="btn ghost"
          disabled={loading}
          onClick={() => load(keyword, day, items.length, true)}
        >
          {loading ? <span className="spinner" /> : "加载更多"}
        </button>
      )}
    </div>
  );
}
