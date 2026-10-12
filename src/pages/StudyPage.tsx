import { useEffect, useMemo, useRef, useState } from "react";
import { api, youdaoAudio } from "../api";
import type { CardDetail, QueueEntry, Settings } from "../types";

interface Props {
  settings: Settings;
  onExit: () => void;
  onToast: (msg: string) => void;
}

/** 与 sm2::Grade 一一对应 */
const STUDY_GRADE = { AGAIN: 0, GOOD: 1, TOO_EASY: 2 } as const;

export default function StudyPage({ settings, onExit, onToast }: Props) {
  const [queue, setQueue] = useState<QueueEntry[]>([]);
  const [index, setIndex] = useState(0);
  const [card, setCard] = useState<CardDetail | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [loading, setLoading] = useState(true);
  const sessionStart = useRef(Date.now());
  /**
   * 本张卡是否已经记过「不认识」。
   * 记过之后这三个按钮里的「我认识 / 太简单」只负责前进，不再重复评分，
   * 否则会把 Again 覆盖成 Good，**丢维当日回队尾重现**的语义。
   */
  const gradedAgain = useRef(false);

  const total = queue.length;
  const current = queue[index] ?? null;

  useEffect(() => {
    let alive = true;
    api
      .buildQueue()
      .then((q) => {
        if (!alive) return;
        setQueue(q);
        setLoading(false);
      })
      .catch((e) => {
        onToast(`读取队列失败：${e}`);
        setLoading(false);
      });
    return () => {
      alive = false;
    };
    // 每次进入学习页只组一次队列
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!current) {
      setCard(null);
      return;
    }
    let alive = true;
    // 换了卡：重置「已记不认识」状态
    gradedAgain.current = false;
    api
      .getCard(current.kind, current.id)
      .then((c) => {
        if (alive) {
          setCard(c);
          setRevealed(false);
        }
      })
      .catch((e) => onToast(`读取卡片失败：${e}`));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.id, current?.kind]);

  // 前台停留时间 = 学习时长
  useEffect(() => {
    const flush = () => {
      const ms = Date.now() - sessionStart.current;
      if (ms > 0) {
        api.addStudyMs(ms).catch(() => undefined);
        sessionStart.current = Date.now();
      }
    };
    const timer = window.setInterval(flush, 5000);
    return () => {
      window.clearInterval(timer);
      flush();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const audio = useMemo(() => {
    if (!card) return null;
    // 单词：优先用 PC 下发的音频 URL，缺失时按美/英偏好拼有道 URL。
    // 句子：按 Q15 用有道整句 TTS（dictvoice?audio=<整句>）。
    if (card.kind === "sentence") {
      return youdaoAudio(card.front, settings.audio_accent);
    }
    return settings.audio_accent === "uk"
      ? card.audio_uk || youdaoAudio(card.front, "uk")
      : card.audio_us || youdaoAudio(card.front, "us");
  }, [card, settings.audio_accent]);

  const play = () => {
    if (!audio) return;
    const el = new Audio(audio);
    el.play().catch(() =>
      onToast("发音加载失败：该词句可能不在有道发音库内，或网络不可用"),
    );
  };

  /** 前进到下一张；`requeue` 表示这张卡记过「不认识」，要挪到队尾（当日重现）。 */
  const advance = (requeue: boolean) => {
    setRevealed(false);
    if (requeue) {
      setQueue((q) => {
        const entry = q[index];
        const rest = q.filter((_, i) => i !== index);
        return entry ? [...rest, entry] : rest;
      });
      // index 不动：移除当前项后，下一项自然顶到该位置
      return;
    }
    if (index + 1 >= total) {
      // 队列走完：落一次学习时长再退出
      api.addStudyMs(Date.now() - sessionStart.current).catch(() => undefined);
      sessionStart.current = Date.now();
      onExit();
      return;
    }
    setIndex((i) => i + 1);
  };

  const grade = async (value: number) => {
    if (!current) return;

    if (value === STUDY_GRADE.AGAIN) {
      // 「不认识」：记 Again + 直接翻开释义，**不前进**
      if (!gradedAgain.current) {
        const wasNew = (card?.state ?? 0) === 0;
        try {
          await api.gradeCard(current.kind, current.id, STUDY_GRADE.AGAIN, wasNew);
        } catch (e) {
          onToast(`保存失败：${e}`);
          return;
        }
        gradedAgain.current = true;
      }
      setRevealed(true);
      return;
    }

    // 「我认识」/「标记为太简单」：评分（若本张已记过 Again 则跳过）并前进
    if (!gradedAgain.current) {
      const wasNew = (card?.state ?? 0) === 0;
      try {
        await api.gradeCard(current.kind, current.id, value, wasNew);
      } catch (e) {
        onToast(`保存失败：${e}`);
        return;
      }
    }
    advance(gradedAgain.current);
  };

  if (loading) {
    return (
      <div className="study">
        <div className="study-card">
          <span className="spinner" />
        </div>
      </div>
    );
  }

  if (!current) {
    return (
      <div className="study">
        <div className="study-head">
          <button className="btn ghost small" onClick={onExit}>
            返回
          </button>
        </div>
        <div className="study-card">
          <div className="study-front">队列已空</div>
          <div className="muted">今天没有待学卡片了</div>
        </div>
        <button className="btn" onClick={onExit}>
          完成
        </button>
      </div>
    );
  }

  const pct = total === 0 ? 0 : Math.round((index / total) * 100);

  return (
    <div className="study">
      <div className="study-head">
        <button className="btn ghost small" onClick={onExit}>
          返回
        </button>
        <div className="bar">
          <i style={{ width: `${pct}%` }} />
        </div>
        <span>
          {Math.min(index + 1, total)}/{total}
        </span>
      </div>

      {/* 空白处不解锁：只有「不认识」才会翻开释义 */}
      <div className="study-card">
        <div className={`study-front ${current.kind === "sentence" ? "sentence" : ""}`}>
          {card?.front ?? current.front}
        </div>

        {card?.kind === "word" && (
          <div className="row" style={{ gap: 12 }}>
            <span className="phonetic">
              {settings.audio_accent === "uk" ? card.phonetic_uk : card.phonetic_us}
            </span>
            <button
              className="btn ghost small"
              onClick={(e) => {
                e.stopPropagation();
                play();
              }}
            >
              🔊
            </button>
          </div>
        )}

        {card?.kind === "sentence" && (
          <button
            className="btn ghost small"
            onClick={(e) => {
              e.stopPropagation();
              play();
            }}
          >
            🔊
          </button>
        )}

        {revealed ? (
          <Gloss card={card} />
        ) : (
          <div className="muted" style={{ marginTop: 10 }}>
            想不起来就点下方「不认识」查看{card?.kind === "sentence" ? "译文" : "释义"}
          </div>
        )}
      </div>

      {revealed ? (
        // 看过释义后不再显示三档评分，只留一个「下一个」
        <button className="btn" onClick={() => advance(gradedAgain.current)}>
          下一个
        </button>
      ) : (
        <>
          <div className="study-actions">
            <button className="btn danger" onClick={() => grade(STUDY_GRADE.AGAIN)}>
              不认识
            </button>
            <button className="btn" onClick={() => grade(STUDY_GRADE.GOOD)}>
              我认识
            </button>
          </div>
          <button
            className="btn ghost"
            style={{ marginTop: 10 }}
            onClick={() => grade(STUDY_GRADE.TOO_EASY)}
          >
            标记为太简单
          </button>
        </>
      )}
    </div>
  );
}

function Gloss({ card }: { card: CardDetail | null }) {
  if (!card) return null;
  if (card.kind === "sentence") {
    return <div className="gloss">{card.trans || "（暂无译文）"}</div>;
  }
  const entries = Array.isArray(card.usual_dict)
    ? (card.usual_dict as { pos?: string; values?: string[] }[])
    : [];
  if (entries.length > 0) {
    return (
      <div className="gloss">
        {entries.map((e, i) => (
          <div key={i}>
            <span className="pos">{e.pos}</span>
            {(e.values ?? []).join("；")}
          </div>
        ))}
      </div>
    );
  }
  return <div className="gloss">{card.dit || "（暂无释义）"}</div>;
}
