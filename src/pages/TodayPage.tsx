import type { Overview, Progress } from "../types";

function fmtMinutes(ms: number): string {
  const min = Math.floor(ms / 60000);
  if (min < 60) return `${min}min`;
  return `${Math.floor(min / 60)}h${min % 60}m`;
}

interface Props {
  progress: Progress;
  overview: Overview;
  sentenceStats: { due: number; fresh: number };
  onStart: () => void;
  /** 进入「速记」：纯单词竖排列表，快速过一遍今天的词 */
  onMemo: () => void;
  syncHint: string | null;
}

export default function TodayPage({
  progress,
  overview,
  sentenceStats,
  onStart,
  onMemo,
  syncHint,
}: Props) {
  const pending = Math.max(0, progress.review_total - progress.review_done);
  const queueEmpty = progress.review_total === 0 && progress.new_total === 0;

  return (
    <div>
      <div className="h1">今日</div>

      <div className="stat-row">
        <div className="stat">
          <div className="num">
            {progress.review_done}
            <small>/{progress.review_total}</small>
          </div>
          <div className="label">今日复习</div>
        </div>
        <div className="stat">
          <div className="num">
            {progress.new_done}
            <small>/{progress.new_total}</small>
          </div>
          <div className="label">今日新词</div>
        </div>
        <div className="stat">
          <div className="num">{fmtMinutes(progress.study_ms)}</div>
          <div className="label">学习时间</div>
        </div>
      </div>

      <div style={{ height: 16 }} />

      <button className="btn" onClick={onStart} disabled={queueEmpty}>
        {queueEmpty ? "今天已完成 🎉" : "开始学习"}
      </button>

      {/* 速记：不进学习流程，只把今天的词竖排过一遍 */}
      <button className="btn ghost" style={{ marginTop: 10 }} onClick={onMemo}>
        速记
      </button>

      <div className="hint">
        待复习 {pending} · 未学 {overview.new_count}
        <br />
        句子：待复习 {sentenceStats.due} · 未学 {sentenceStats.fresh}
      </div>

      {syncHint && <div className="hint">{syncHint}</div>}

      <div className="card" style={{ marginTop: 14 }}>
        <div className="row between">
          <span className="muted">本地词库</span>
          <span>
            {overview.word_count} 词 · {overview.sentence_count} 句
          </span>
        </div>
        <div className="row between" style={{ marginTop: 8 }}>
          <span className="muted">同步游标</span>
          <span>seq {overview.last_seq}</span>
        </div>
      </div>
    </div>
  );
}
