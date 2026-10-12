import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { BrowseItem } from "../types";

const PAGE_SIZE = 40;

interface Props {
  onToast: (msg: string) => void;
}

export default function SentencesPage({ onToast }: Props) {
  const [keyword, setKeyword] = useState("");
  const [items, setItems] = useState<BrowseItem[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const reqId = useRef(0);

  const load = useCallback(
    async (kw: string, offset: number, append: boolean) => {
      const id = ++reqId.current;
      setLoading(true);
      try {
        const page = await api.browseSentences(kw, offset, PAGE_SIZE);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  useEffect(() => {
    const t = window.setTimeout(() => load(keyword, 0, false), keyword ? 250 : 0);
    return () => window.clearTimeout(t);
  }, [keyword, load]);

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

  return (
    <div>
      <div className="h1">句子本</div>

      <input
        placeholder="搜索句子或译文"
        value={keyword}
        onChange={(e) => setKeyword(e.target.value)}
      />

      <div className="muted" style={{ margin: "10px 0 6px" }}>
        共 {total} 条
      </div>

      <div className="card" style={{ padding: "4px 14px" }}>
        {items.length === 0 && !loading && <div className="empty">句子本还是空的</div>}
        {items.map((it) => (
          <div className="list-item" key={it.id}>
            <div className="main">
              <div className="title" style={{ whiteSpace: "normal" }}>
                {it.text}
              </div>
              <div className="sub" style={{ whiteSpace: "normal" }}>
                {it.gloss || "—"}
              </div>
            </div>
            <button className="btn danger small" onClick={() => remove(it.id)}>
              删除
            </button>
          </div>
        ))}
      </div>

      {hasMore && (
        <button className="btn ghost" disabled={loading} onClick={() => load(keyword, items.length, true)}>
          {loading ? <span className="spinner" /> : "加载更多"}
        </button>
      )}
    </div>
  );
}
