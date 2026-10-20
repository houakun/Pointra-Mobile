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
/** 左右滑超过这个距离就切上/下一张 */
const SWIPE_PX = 60;
/** 判定主轴的最小位移：小于它就当成「点按」 */
const AXIS_PX = 10;
/**
 * 屏幕左右边缘留出的宽度：从边缘起手的滑动是**系统返回手势**（向内滑），
 * 不参与切卡 —— 否则用户想退出学习页，结果变成了“上一张/下一张”，
 * 连顶部进度都跟着变。
 */
const EDGE_PX = 28;

/**
 * 学习位置（当日最近看到的那张卡）。
 *
 * 退出学习页再点「开始学习」时，队列会重新组装（已评分的卡不再入队），
 * 只靠 index 就会从第一张重来。记住这张卡的 id，回来时把 index 放到它身上。
 */
const STUDY_POS_KEY = "pointra:study-pos:v1";

/** 本地日期键，只用来判断「是不是同一天」 */
function todayKey(): string {
  const d = new Date();
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
}

function readStudyPos(): { day: string; kind: string; id: string } | null {
  try {
    const raw = localStorage.getItem(STUDY_POS_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return null;
    const pos = parsed as { day?: unknown; kind?: unknown; id?: unknown };
    if (
      typeof pos.day !== "string" ||
      typeof pos.kind !== "string" ||
      typeof pos.id !== "string"
    ) {
      return null;
    }
    return { day: pos.day, kind: pos.kind, id: pos.id };
  } catch {
    return null;
  }
}

function saveStudyPos(entry: QueueEntry) {
  try {
    localStorage.setItem(
      STUDY_POS_KEY,
      JSON.stringify({ day: todayKey(), kind: entry.kind, id: entry.id }),
    );
  } catch {
    // 存不下也不影响学习
  }
}

export default function StudyPage({ settings, onExit, onToast }: Props) {
  const [queue, setQueue] = useState<QueueEntry[]>([]);
  const [index, setIndex] = useState(0);
  const [card, setCard] = useState<CardDetail | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [loading, setLoading] = useState(true);
  /** 拖动中的水平位移（跟手反馈） */
  const [dragX, setDragX] = useState(0);
  /** 切卡动画方向 */
  const [anim, setAnim] = useState<"left" | "right" | null>(null);
  const sessionStart = useRef(Date.now());
  /**
   * 本张卡是否已经记过「不认识」。
   * 记过之后「我认识 / 太简单」只负责前进，不再重复评分，
   * 否则会把 Again 覆盖成 Good，**丢维当日回队尾重现**的语义。
   */
  const gradedAgain = useRef(false);
  const gesture = useRef<{
    x: number;
    y: number;
    dx: number;
    dy: number;
    axis: "none" | "x" | "y";
  } | null>(null);
  /** 滑动结束后浏览器还会补发一次 click，别让它误触按钮评分 */
  const swallowClick = useRef(false);

  const total = queue.length;
  const current = queue[index] ?? null;

  /**
   * 「进入本轮之前，今天已经学完多少张」。
   *
   * 退出学习页再进来时队列会重新组装（已评分的卡不再入队），
   * 光看本轮 index 会从 0 开始，看起来像“进度全丢了”。
   * 拿今日已完成量做基线，进度条就接得上（5/20 → 继续）。
   */
  const [doneBefore, setDoneBefore] = useState(0);
  /**
   * 本轮已经评分过的张数。
   *
   * 进度只跟它走：左右滑动只是「看一眼别的卡」，并没有学，不该让进度条动。
   */
  const [graded, setGraded] = useState(0);

  useEffect(() => {
    let alive = true;
    Promise.all([api.buildQueue(), api.getProgress()])
      .then(([q, p]) => {
        if (!alive) return;
        setQueue(q);
        setDoneBefore(p.review_done + p.new_done);
        // 回到上次看到的那张卡（同一天、且它还在队列里）
        const saved = readStudyPos();
        if (saved && saved.day === todayKey()) {
          const pos = q.findIndex((e) => e.kind === saved.kind && e.id === saved.id);
          if (pos > 0) setIndex(pos);
        }
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
    // 记住「现在看到哪张」：退出再进来能接着这张，而不是从第一张重来
    saveStudyPos(current);
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

  useEffect(() => {
    if (!anim) return;
    const t = window.setTimeout(() => setAnim(null), 280);
    return () => window.clearTimeout(t);
  }, [anim]);

  const audio = useMemo(() => {
    if (!card) return null;
    if (card.kind === "sentence") {
      return youdaoAudio(card.front, settings.audio_accent);
    }
    return settings.audio_accent === "uk"
      ? card.audio_uk || youdaoAudio(card.front, "uk")
      : card.audio_us || youdaoAudio(card.front, "us");
  }, [card, settings.audio_accent]);

  const play = () => {
    if (!audio) return;
    new Audio(audio)
      .play()
      .catch(() =>
        onToast("发音加载失败：该词句可能不在有道发音库内，或网络不可用"),
      );
  };

  /** 左右滑动：只切上/下一张，**不评分**（评分交给三个按钮）。 */
  const goTo = (delta: 1 | -1) => {
    const target = index + delta;
    if (target < 0 || target >= total) return;
    setRevealed(false);
    setAnim(delta === 1 ? "left" : "right");
    setIndex(target);
  };

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
        // 这张算学过了：进度条 +1（滑动浏览不加）
        setGraded((n) => n + 1);
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
      // 评分成功才算学完一张（Again 已计过，不重复加）
      setGraded((n) => n + 1);
    }
    advance(gradedAgain.current);
  };

  // 不给容器 setPointerCapture：否则 click 会被重定向到容器，按钮点不动。
  // 容器是全屏的，不捕获也能拿到冒泡上来的 pointerup。
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    // 从屏幕边缘起手的是系统返回手势，本条手势不参与切卡（系统会关掉学习页）
    const fromEdge = e.clientX < EDGE_PX || e.clientX > window.innerWidth - EDGE_PX;
    gesture.current = fromEdge
      ? null
      : { x: e.clientX, y: e.clientY, dx: 0, dy: 0, axis: "none" };
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const g = gesture.current;
    if (!g) return;
    g.dx = e.clientX - g.x;
    g.dy = e.clientY - g.y;
    if (g.axis === "none" && (Math.abs(g.dx) > AXIS_PX || Math.abs(g.dy) > AXIS_PX)) {
      g.axis = Math.abs(g.dx) > Math.abs(g.dy) ? "x" : "y";
    }
    if (g.axis === "x") setDragX(g.dx);
  };

  const onPointerUp = () => {
    const g = gesture.current;
    gesture.current = null;
    setDragX(0);
    if (!g) return;
    if (g.axis === "x" && Math.abs(g.dx) >= SWIPE_PX) {
      swallowClick.current = true;
      window.setTimeout(() => {
        swallowClick.current = false;
      }, 350);
      goTo(g.dx < 0 ? 1 : -1);
    }
  };

  const onPointerCancel = () => {
    gesture.current = null;
    setDragX(0);
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
          <div className="bar">
            <i style={{ width: "100%" }} />
          </div>
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

  // 进度只跟「已评分张数」走：左右滑动浏览别的卡，进度条不动
  const grandTotal = doneBefore + total;
  const doneTotal = doneBefore + graded;
  const pct = grandTotal === 0 ? 0 : Math.round((doneTotal / grandTotal) * 100);

  return (
    <div
      className="study"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onClickCapture={(e) => {
        // 滑动之后浏览器补发的那次 click 直接吞掉，避免误评分
        if (swallowClick.current) {
          e.preventDefault();
          e.stopPropagation();
        }
      }}
    >
      <div className="study-head">
        <div className="bar">
          <i style={{ width: `${pct}%` }} />
        </div>
        <span>
          {Math.min(doneTotal + 1, grandTotal)}/{grandTotal}
        </span>
      </div>

      {/* 左右滑动切换上/下一张：拖动跟手，松手过阈值即切 */}
      <div
        className={`study-card${dragX !== 0 ? " dragging" : ""}${
          anim ? ` anim-${anim}` : ""
        }`}
        style={dragX !== 0 ? { transform: `translateX(${dragX}px)` } : undefined}
      >
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
            style={{ marginTop: 8 }}
            onClick={() => grade(STUDY_GRADE.TOO_EASY)}
          >
            标记为太简单
          </button>
        </>
      )}

      <div className="study-hint muted">← 左右滑动切换单词 · 从屏幕边缘滑动返回 →</div>
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
