import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { CardDetail, QueueEntry } from "../types";

/** 左滑超过这个距离就清除该词 */
const CLEAR_PX = 60;
/** 与 `sm2::Grade::TooEasy` 对应：左滑清除 = 标记为太简单 */
const TOO_EASY = 2;
/** 判定主轴的最小位移：小于它就当成「点按」 */
const AXIS_PX = 10;

interface Props {
  onExit: () => void;
  onToast: (msg: string) => void;
}

type Gesture = {
  /** 按下时落在哪一行（横向滑动用） */
  id: string | null;
  x: number;
  y: number;
  dx: number;
  dy: number;
  axis: "none" | "x" | "y";
};

export default function MemoPage({ onExit, onToast }: Props) {
  const [words, setWords] = useState<QueueEntry[]>([]);
  const [loading, setLoading] = useState(true);
  /**
   * 已展开释义的词 id。【数组而不是单个 id】：
   * 点过的词释义要**一直留着**，可以同时展开多张，
   * 再点自己才收起；只有左滑清除才会让它消失。
   */
  const [expanded, setExpanded] = useState<string[]>([]);
  const [defs, setDefs] = useState<Record<string, CardDetail>>({});
  const [removed, setRemoved] = useState<string[]>([]);
  const [leaving, setLeaving] = useState<string | null>(null);
  const [drag, setDrag] = useState<{ id: string; dx: number } | null>(null);

  const gesture = useRef<Gesture | null>(null);

  // 速记的就是「今天要学/要复习」的那批词（与学习页同源），只取单词
  useEffect(() => {
    let alive = true;
    api
      .buildQueue()
      .then((queue) => {
        if (!alive) return;
        setWords(queue.filter((entry) => entry.kind === "word"));
        setLoading(false);
      })
      .catch((e) => {
        onToast(`读取队列失败：${e}`);
        setLoading(false);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const visible = useMemo(
    () => words.filter((w) => !removed.includes(w.id)),
    [words, removed],
  );

  const openWord = (entry: QueueEntry) => {
    // 每个词各自独立：只切换自己，不动别人已展开的释义
    setExpanded((prev) =>
      prev.includes(entry.id)
        ? prev.filter((id) => id !== entry.id)
        : [...prev, entry.id],
    );
    if (!defs[entry.id]) {
      void (async () => {
        try {
          const card = await api.getCard(entry.kind, entry.id);
          setDefs((prev) => ({ ...prev, [entry.id]: card }));
        } catch (e) {
          onToast(`读取释义失败：${e}`);
        }
      })();
    }
  };

  /**
   * 左滑 = **标记为太简单**：先写回学习进度（state=3、不再入队），
   * 成功后再把它从本次速记列表里滑出，避免「界面清掉了但没落库」。
   */
  const clearWord = async (id: string) => {
    const entry = words.find((w) => w.id === id);
    if (!entry) return;
    try {
      await api.gradeCard(entry.kind, entry.id, TOO_EASY, entry.state === 0);
    } catch (e) {
      onToast(`清除失败：${e}`);
      return;
    }
    setLeaving(id);
    window.setTimeout(() => {
      setRemoved((prev) => [...prev, id]);
      // 行都清掉了，它的展开状态也一并收掉（否则以后再出现会莫名其妙展开）
      setExpanded((prev) => prev.filter((x) => x !== id));
      setLeaving(null);
      onToast("已标记为太简单");
    }, 190);
  };

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    const rowEl = (e.target as HTMLElement).closest("[data-word-id]") as HTMLElement | null;
    gesture.current = {
      id: rowEl?.dataset.wordId ?? null,
      x: e.clientX,
      y: e.clientY,
      dx: 0,
      dy: 0,
      axis: "none",
    };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const g = gesture.current;
    if (!g) return;
    g.dx = e.clientX - g.x;
    g.dy = e.clientY - g.y;
    if (g.axis === "none" && (Math.abs(g.dx) > AXIS_PX || Math.abs(g.dy) > AXIS_PX)) {
      // 一旦定了方向就不再改，避免斜着滑时来回跳
      g.axis = Math.abs(g.dx) > Math.abs(g.dy) ? "x" : "y";
    }
    // 竖直方向交给浏览器原生滚动（CSS touch-action: pan-y），这里只管横向
    if (g.axis === "x" && g.id && g.dx < 0) {
      setDrag({ id: g.id, dx: Math.max(g.dx, -170) });
    }
  };

  /** 浏览器开始接管竖直滚动时会发 pointercancel：只收尾，不能当点按处理。 */
  const onPointerCancel = () => {
    gesture.current = null;
    setDrag(null);
  };

  const onPointerUp = () => {
    const g = gesture.current;
    gesture.current = null;
    if (!g) return;
    setDrag(null);

    // 没怎么动 → 当成点按：展开/收起释义
    if (g.axis === "none") {
      if (g.id) {
        const entry = words.find((w) => w.id === g.id);
        if (entry) openWord(entry);
      }
      return;
    }
    if (g.axis === "x" && g.id && g.dx <= -CLEAR_PX) {
      void clearWord(g.id);
    }
  };

  return (
    <div className="memo">
      <div className="memo-head">
        <button className="btn ghost small" onClick={onExit}>
          返回
        </button>
        <div className="memo-title">速记</div>
        <span className="muted">{visible.length} 词</span>
      </div>

      <div
        className="memo-body"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerCancel}
      >
        {loading && (
          <div className="empty">
            <span className="spinner" />
          </div>
        )}

        {!loading && visible.length === 0 && (
          <div className="empty">今天没有要速记的单词</div>
        )}

        {visible.map((w) => {
          const isOpen = expanded.includes(w.id);
          const isLeaving = leaving === w.id;
          const dx = drag?.id === w.id ? drag.dx : 0;
          return (
            <div className="memo-row" key={w.id} data-word-id={w.id}>
              {dx < -12 && <div className="memo-clear-hint">清除</div>}
              <div
                className={`memo-row-inner${dx !== 0 ? " dragging" : ""}${
                  isLeaving ? " leaving" : ""
                }`}
                style={{ transform: `translateX(${isLeaving ? "-120%" : `${dx}px`})` }}
              >
                <div className="memo-word">{w.front}</div>
                {isOpen && <div className="memo-def">{renderDef(defs[w.id])}</div>}
              </div>
            </div>
          );
        })}

        {!loading && visible.length > 0 && (
          <div className="memo-end muted">— 到底了 —</div>
        )}
      </div>

      <div className="memo-hint muted">
        上下滚动浏览 · 点单词看释义 · 向左滑动＝标记太简单
      </div>
    </div>
  );
}

/** 常见释义：优先按词性分行，取不到再退回 dit / 译文。 */
function renderDef(card?: CardDetail) {
  if (!card) return <span className="muted">…</span>;
  if (card.kind === "sentence") return card.trans || "（暂无译文）";
  const entries = Array.isArray(card.usual_dict)
    ? (card.usual_dict as { pos?: string; values?: string[] }[])
    : [];
  if (entries.length > 0) {
    return (
      <>
        {entries.map((entry, i) => (
          <div key={i}>
            <span className="pos">{entry.pos}</span>
            {(entry.values ?? []).join("；")}
          </div>
        ))}
      </>
    );
  }
  return card.dit || "（暂无释义）";
}
