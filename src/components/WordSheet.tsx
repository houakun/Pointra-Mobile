import { useState } from "react";
import { api, youdaoAudio } from "../api";
import { EXCHANGE_LABEL } from "../labels";
import type { Accent, WordLookup } from "../types";

interface Props {
  loading: boolean;
  /** 查词失败的中文提示（Rust 侧已翻译成人话） */
  error: string | null;
  lookup: WordLookup | null;
  accent: Accent;
  onClose: () => void;
  onToast: (msg: string) => void;
  /** 收藏成功后通知外层刷新统计（今日/词库数字） */
  onAdded: () => void;
}

/**
 * 阅读页的底部词卡：点文中任意单词弹出。
 *
 * 与 `WordDetailPage` 的区别：这里的词来自「即时查词」，
 * 不是本机词库里的记录，所以没有学习状态、没有加入日期，
 * 而是多一块「文中」——该词所在句子的译文（成功标准 2）。
 */
export default function WordSheet({
  loading,
  error,
  lookup,
  accent,
  onClose,
  onToast,
  onAdded,
}: Props) {
  const [saving, setSaving] = useState(false);

  const play = (which: Accent) => {
    if (!lookup) return;
    new Audio(youdaoAudio(lookup.word, which))
      .play()
      .catch(() => onToast("发音加载失败：可能网络不可用"));
  };

  const add = async () => {
    if (!lookup) return;
    setSaving(true);
    try {
      const result = await api.addLocalWord(lookup.word);
      onToast(
        result.existed
          ? `「${result.word}」已在单词本里，已刷新词义`
          : `已加入单词本：${result.word}`,
      );
      onAdded();
    } catch (e) {
      onToast(`加入失败：${e}`);
    } finally {
      setSaving(false);
    }
  };

  const exchange =
    lookup?.exchange && typeof lookup.exchange === "object"
      ? Object.entries(lookup.exchange).filter(([, values]) =>
          Array.isArray(values) ? values.length > 0 : Boolean(values),
        )
      : [];

  return (
    <div className="sheet-mask" onClick={onClose}>
      <div className="sheet" onClick={(e) => e.stopPropagation()}>
        <div className="sheet-grip" />

        {loading && (
          <div className="empty">
            <span className="spinner" />
          </div>
        )}

        {!loading && error && (
          <div className="sheet-error">{error}</div>
        )}

        {!loading && lookup && (
          <div className="sheet-body">
            <div className="sheet-head">
              <div className="sheet-word">{lookup.word}</div>
              <button className="btn ghost small" onClick={onClose} aria-label="关闭">
                ✕
              </button>
            </div>

            {(lookup.phonetic_uk || lookup.phonetic_us) && (
              <div className="sheet-phon">
                {lookup.phonetic_uk && (
                  <button
                    className={`phon${accent === "uk" ? " on" : ""}`}
                    onClick={() => play("uk")}
                  >
                    🔊 英 /{lookup.phonetic_uk}/
                  </button>
                )}
                {lookup.phonetic_us && (
                  <button
                    className={`phon${accent === "us" ? " on" : ""}`}
                    onClick={() => play("us")}
                  >
                    🔊 美 /{lookup.phonetic_us}/
                  </button>
                )}
              </div>
            )}

            <div className="detail-card">
              <div className="detail-card-title">释义</div>
              {lookup.usual_dict.length > 0 ? (
                lookup.usual_dict.map((group, i) => (
                  <div className="detail-gloss" key={i}>
                    <span className="pos">{group.pos}</span>
                    {group.values.join("；")}
                  </div>
                ))
              ) : (
                <div className="detail-gloss">{lookup.dit || "（暂无释义）"}</div>
              )}
            </div>

            {(lookup.sentence || lookup.sentence_trans || lookup.sentence_error) && (
              <div className="detail-card">
                <div className="detail-card-title">文中</div>
                {lookup.sentence && <div className="sheet-sentence">{lookup.sentence}</div>}
                {lookup.sentence_trans && (
                  <div className="sheet-trans">{lookup.sentence_trans}</div>
                )}
                {lookup.sentence_error && (
                  <div className="sheet-hint">句中译文没取到：{lookup.sentence_error}</div>
                )}
              </div>
            )}

            {exchange.length > 0 && (
              <div className="detail-card">
                <div className="detail-card-title">词形变化</div>
                {exchange.map(([key, values]) => (
                  <div className="detail-exchange" key={key}>
                    <span className="pos">{EXCHANGE_LABEL[key] ?? key}</span>
                    {(Array.isArray(values) ? values : [String(values)]).join("、")}
                  </div>
                ))}
              </div>
            )}

            <button className="btn primary wide" onClick={add} disabled={saving}>
              {saving ? "加入中…" : "＋ 加入单词本"}
            </button>

            {lookup.cached && <div className="sheet-hint">（释义来自本机缓存）</div>}
          </div>
        )}
      </div>
    </div>
  );
}
