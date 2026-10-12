import { useEffect, useMemo, useState } from "react";
import { api, youdaoAudio } from "../api";
import type { Accent, CardDetail, CardKind } from "../types";

/** 词形变化的中文标签（有道 exchange 字段的键） */
const EXCHANGE_LABEL: Record<string, string> = {
  word_pl: "复数",
  word_past: "过去式",
  word_done: "过去分词",
  word_ing: "现在分词",
  word_third: "第三人称单数",
  word_er: "比较级",
  word_est: "最高级",
};

const STATE_LABEL: Record<number, string> = {
  0: "未学",
  1: "学习中",
  2: "复习",
  3: "已掌握",
};

interface Props {
  id: string;
  kind: CardKind;
  accent: Accent;
  onClose: () => void;
  onToast: (msg: string) => void;
}

/**
 * 单词/句子详情页（词库列表点进来）。
 *
 * 数据来自 PC 端下发的 `usual_dict` / `dit` / `exchange`；
 * 与「不背单词」那种详情页相比，**没有例句/真题例句**——
 * PC 端生词库只存了释义与词形变化，没有抓例句，这里如实只展示已有的部分。
 */
export default function WordDetailPage({
  id,
  kind,
  accent,
  onClose,
  onToast,
}: Props) {
  const [card, setCard] = useState<CardDetail | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    api
      .getCard(kind, id)
      .then((c) => {
        if (!alive) return;
        setCard(c);
        setLoading(false);
      })
      .catch((e) => {
        onToast(`读取详情失败：${e}`);
        setLoading(false);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, kind]);

  const phonetic = useMemo(() => {
    if (!card || card.kind !== "word") return "";
    return (accent === "uk" ? card.phonetic_uk : card.phonetic_us) ?? "";
  }, [card, accent]);

  const audio = useMemo(() => {
    if (!card) return null;
    if (card.kind === "sentence") return youdaoAudio(card.front, accent);
    return accent === "uk"
      ? card.audio_uk || youdaoAudio(card.front, "uk")
      : card.audio_us || youdaoAudio(card.front, "us");
  }, [card, accent]);

  const play = () => {
    if (!audio) return;
    new Audio(audio)
      .play()
      .catch(() => onToast("发音加载失败：该词句可能不在有道发音库内，或网络不可用"));
  };

  const entries = Array.isArray(card?.usual_dict)
    ? (card?.usual_dict as { pos?: string; values?: string[] }[])
    : [];
  const exchange =
    card && card.exchange && typeof card.exchange === "object" && !Array.isArray(card.exchange)
      ? (card.exchange as Record<string, string[]>)
      : null;

  return (
    <div className="detail">
      <div className="detail-head">
        <button className="btn ghost small" onClick={onClose}>
          ← 返回
        </button>
        <span className={`pill s${card?.state ?? 0}`}>
          {STATE_LABEL[card?.state ?? 0] ?? "—"}
        </span>
      </div>

      <div className="detail-body">
        {loading && (
          <div className="empty">
            <span className="spinner" />
          </div>
        )}

        {!loading && card && (
          <>
            <div className="detail-word">{card.front}</div>

            <div className="detail-sub">
              {phonetic && <span className="phonetic">{phonetic}</span>}
              <button
                className="btn ghost small"
                onClick={play}
                aria-label="发音"
              >
                🔊
              </button>
            </div>

            <div className="detail-card">
              <div className="detail-card-title">
                {card.kind === "sentence" ? "译文" : "释义"}
              </div>
              {card.kind === "sentence" ? (
                <div className="detail-gloss">{card.trans || "（暂无译文）"}</div>
              ) : entries.length > 0 ? (
                entries.map((entry, i) => (
                  <div className="detail-gloss" key={i}>
                    <span className="pos">{entry.pos}</span>
                    {(entry.values ?? []).join("；")}
                  </div>
                ))
              ) : (
                <div className="detail-gloss">{card.dit || "（暂无释义）"}</div>
              )}
            </div>

            {exchange && Object.keys(exchange).length > 0 && (
              <div className="detail-card">
                <div className="detail-card-title">词形变化</div>
                {Object.entries(exchange).map(([key, values]) => (
                  <div className="detail-exchange" key={key}>
                    <span className="pos">{EXCHANGE_LABEL[key] ?? key}</span>
                    {(Array.isArray(values) ? values : [String(values)]).join("、")}
                  </div>
                ))}
              </div>
            )}

            <div className="detail-meta muted">
              {card.added_at
                ? `加入日期：${new Date(card.added_at).toLocaleDateString()}`
                : ""}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
