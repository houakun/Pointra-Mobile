import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { CardKind, Overview, Progress, Settings } from "./types";
import { useBackGuard } from "./hooks/useBackGuard";
import TodayPage from "./pages/TodayPage";
import StudyPage from "./pages/StudyPage";
import MemoPage from "./pages/MemoPage";
import WordDetailPage from "./pages/WordDetailPage";
import VocabPage from "./pages/VocabPage";
import SentencesPage from "./pages/SentencesPage";
import SettingsPage from "./pages/SettingsPage";

type Tab = "today" | "vocab" | "sentences" | "settings";

const EMPTY_OVERVIEW: Overview = {
  word_count: 0,
  sentence_count: 0,
  new_count: 0,
  due_count: 0,
  last_seq: 0,
};

const EMPTY_PROGRESS: Progress = {
  review_done: 0,
  review_total: 0,
  new_done: 0,
  new_total: 0,
  study_ms: 0,
};

const DEFAULT_SETTINGS: Settings = {
  pairing: null,
  goals: { new_per_day: 20, review_per_day: 100 },
  audio_accent: "us",
  last_sync_at: null,
};

export default function App() {
  const [tab, setTab] = useState<Tab>("today");
  const [studying, setStudying] = useState(false);
  const [memo, setMemo] = useState(false);
  /** 词库/句子本点进来的详情页 */
  const [detail, setDetail] = useState<{ id: string; kind: CardKind } | null>(null);
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [overview, setOverview] = useState<Overview>(EMPTY_OVERVIEW);
  const [progress, setProgress] = useState<Progress>(EMPTY_PROGRESS);
  const [toast, setToast] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const toastTimer = useRef<number | null>(null);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2600);
  }, []);

  const refresh = useCallback(async () => {
    const [o, p] = await Promise.all([api.getOverview(), api.getProgress()]);
    setOverview(o);
    setProgress(p);
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const s = await api.getSettings();
        setSettings(s);
        await refresh();
        if (s.pairing) {
          // 冷启动：先增量补拉，再起 WS（P5）
          api.syncNow().catch(() => undefined).then(refresh);
          api.startEvents().catch(() => undefined);
        }
      } catch (e) {
        showToast(`初始化失败：${e}`);
      } finally {
        setReady(true);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // PC 端 WS 推送：收到就刷新统计并提示
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<{ type: string; seq: number }>("sync://added", (ev) => {
      showToast(
        ev.payload.type === "sentence.added" ? "句子本 +1" : "生词本 +1",
      );
      refresh();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [refresh, showToast]);

  // WS 重连成功（含首次连上）→ 立即增量补拉，补齐断线期间错过的记录。
  // 这就是「手机一打开/网络一恢复，词就齐了」的实现点。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("sync://reconnected", () => {
      api
        .syncNow()
        .then(() => refresh())
        .catch(() => undefined);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [refresh]);

  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refresh]);

  // 从设置页改完「每日上限」再回到今日页时，必须重新拉一次统计，
  // 否则会看到改动前的旧分母（看起来像「没生效」）。
  useEffect(() => {
    if (tab === "today") refresh();
  }, [tab, refresh]);

  // 覆盖页打开时，Android 返回键 / 全面屏边缘手势先关掉它，而不是退出整个 App。
  // 根页面（今日/词库/…）没有历史可回，则保持系统惯例（退出 App）。
  const overlayOpen = studying || memo || detail !== null;
  useBackGuard(
    overlayOpen,
    useCallback(() => {
      // 同一时刻只会有一个覆盖页，按最上层往关
      if (detail) return setDetail(null);
      if (memo) return setMemo(false);
      setStudying(false);
      refresh();
    }, [detail, memo, refresh]),
  );

  // 启动时静默问一次电脑端有没有新版本（失败就算了，不打扰用户）
  useEffect(() => {
    if (!ready || !settings.pairing?.base) return;
    api
      .checkUpdate()
      .then((info) => {
        if (info?.newer) showToast(`发现新版本 v${info.version}，去设置页更新`);
      })
      .catch(() => undefined);
  }, [ready, settings.pairing?.base, showToast]);

  if (!ready) {
    return (
      <div className="app">
        <div className="app-body">
          <div className="empty">
            <span className="spinner" />
          </div>
        </div>
      </div>
    );
  }

  const sentenceStats = {
    due: Math.max(0, progress.review_total - progress.review_done),
    fresh: overview.sentence_count,
  };

  return (
    <div className="app">
      <div className="app-body">
        {tab === "today" && (
          <TodayPage
            progress={progress}
            overview={overview}
            sentenceStats={sentenceStats}
            onStart={() => setStudying(true)}
            onMemo={() => setMemo(true)}
            syncHint={settings.pairing ? `同步：${settings.pairing.base}` : "未配对，去设置页配对"}
          />
        )}
        {tab === "vocab" && (
          <VocabPage
            onToast={showToast}
            onOpen={(id, kind) => setDetail({ id, kind })}
          />
        )}
        {tab === "sentences" && (
          <SentencesPage
            onToast={showToast}
            onOpen={(id, kind) => setDetail({ id, kind })}
          />
        )}
        {tab === "settings" && (
          <SettingsPage settings={settings} onChange={setSettings} onToast={showToast} />
        )}
      </div>

      {studying && (
        <StudyPage
          settings={settings}
          onExit={() => {
            setStudying(false);
            refresh();
          }}
          onToast={showToast}
        />
      )}

      {memo && <MemoPage onExit={() => setMemo(false)} onToast={showToast} />}

      {detail && (
        <WordDetailPage
          id={detail.id}
          kind={detail.kind}
          accent={settings.audio_accent}
          onClose={() => setDetail(null)}
          onToast={showToast}
        />
      )}

      <nav className="tabbar">
        <TabButton icon="📅" label="今日" active={tab === "today"} onClick={() => setTab("today")} />
        <TabButton icon="📚" label="词库" active={tab === "vocab"} onClick={() => setTab("vocab")} />
        <TabButton
          icon="💬"
          label="句子"
          active={tab === "sentences"}
          onClick={() => setTab("sentences")}
        />
        <TabButton icon="⚙️" label="设置" active={tab === "settings"} onClick={() => setTab("settings")} />
      </nav>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function TabButton({
  icon,
  label,
  active,
  onClick,
}: {
  icon: string;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button className={active ? "active" : ""} onClick={onClick}>
      <span className="icon">{icon}</span>
      <span>{label}</span>
    </button>
  );
}
