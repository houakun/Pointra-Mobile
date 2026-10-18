import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { ArticlePicks as Picks } from "../types";

interface Props {
  articleId: string;
  articleTitle: string;
  onClose: () => void;
  onToast: (msg: string) => void;
}

/**
 * 「本篇速记」：这篇文章里我收藏过的生词与生句。
 *
 * 与「词库 / 句子本」的区别：那两个是**全书**维度，这里是**单篇**维度，
 * 用来回看「我刚读的这篇里我挑了什么」。
 *
 * 呈现方式跟句子本一致：一条一行，点一下就地展开释义 / 译文。
 * **不提供删除**（要删去词库 / 句子本里删，这里只用来速览）。
 */
export default function ArticlePicks({
  articleId,
  articleTitle,
  onClose,
  onToast,
}: Props) {
  const [picks, setPicks] = useState<Picks | null>(null);
  const [loading, setLoading] = useState(true);
  /** 展开的生词 id（一次一条，列表才不会被撑得很长） */
  const [openWord, setOpenWord] = useState<string | null>(null);
  /** 展开的生句 id */
  const [openSentence, setOpenSentence] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setPicks(await api.listArticlePicks(articleId));
    } catch (e) {
      onToast(`读取本篇速记失败：${e}`);
    } finally {
      setLoading(false);
    }
  }, [articleId, onToast]);

  useEffect(() => {
    void load();
  }, [load]);

  const words = picks?.words ?? [];
  const sentences = picks?.sentences ?? [];
  const empty = !loading && words.length === 0 && sentences.length === 0;

  return (
    <div className="picks">
      <div className="picks-head">
        <button className="btn ghost small" onClick={onClose}>
          ← 返回
        </button>
        <div className="picks-title">本篇速记</div>
        <span className="muted">
          {loading ? "…" : `${words.length} 词 · ${sentences.length} 句`}
        </span>
      </div>

      <div className="picks-sub muted">{articleTitle}</div>

      <div className="picks-body">
        {loading && (
          <div className="empty">
            <span className="spinner" />
          </div>
        )}

        {empty && (
          <div className="empty">
            这篇还没有收藏。
            <br />
            阅读时点单词右侧的 ☆ 加入单词本、选中句子后点「＋ 句子本」。
          </div>
        )}

        {words.length > 0 && (
          <>
            <div className="picks-group">生词 · {words.length}</div>
            <div className="card" style={{ padding: "4px 14px" }}>
              {words.map((w) => (
                <div className="list-item" key={w.id}>
                  <div
                    className="main tappable"
                    onClick={() => setOpenWord(openWord === w.id ? null : w.id)}
                  >
                    <div className="title picks-word">
                      {w.word}
                      {(w.phonetic_us || w.phonetic_uk) && (
                        <span className="picks-phon muted">
                          /{w.phonetic_us ?? w.phonetic_uk}/
                        </span>
                      )}
                    </div>
                    {openWord === w.id && (
                      <div className="sub" style={{ whiteSpace: "normal" }}>
                        {w.usual_dict.length > 0
                          ? w.usual_dict.map((group, i) => (
                              <div className="detail-gloss" key={i}>
                                <span className="pos">{group.pos}</span>
                                {group.values.join("；")}
                              </div>
                            ))
                          : (w.dit ?? "（暂无释义）")}
                      </div>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </>
        )}

        {sentences.length > 0 && (
          <>
            <div className="picks-group">生句 · {sentences.length}</div>
            <div className="card" style={{ padding: "4px 14px" }}>
              {sentences.map((s) => (
                <div className="list-item" key={s.id}>
                  <div
                    className="main tappable"
                    onClick={() =>
                      setOpenSentence(openSentence === s.id ? null : s.id)
                    }
                  >
                    <div className="title" style={{ whiteSpace: "normal" }}>
                      {s.text}
                    </div>
                    {openSentence === s.id && (
                      <div className="sub" style={{ whiteSpace: "normal" }}>
                        {s.trans || "（暂无译文）"}
                      </div>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </>
        )}

        <div style={{ height: 20 }} />
      </div>
    </div>
  );
}
