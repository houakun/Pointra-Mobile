import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type {
  Accent,
  Settings,
  SyncProgress,
  Theme,
  UpdateInfo,
  UpdateProgress,
  WordCardSide,
} from "../types";

interface Props {
  settings: Settings;
  onChange: (s: Settings) => void;
  onToast: (msg: string) => void;
}

export default function SettingsPage({ settings, onChange, onToast }: Props) {
  const [base, setBase] = useState(settings.pairing?.base ?? "10.0.2.2:31415");
  // 旧安装的 settings.json 里没这个字段，读出来是 undefined，统一按「自动」展示
  const cardSide: WordCardSide = settings.word_card_side ?? "auto";
  const [code, setCode] = useState("");
  const [deviceName, setDeviceName] = useState("Android");
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<SyncProgress | null>(null);
  /**
   * 版本号：走**自己的** Rust 命令读编译进二进制的包版本。
   *
   * 之前用 `getVersion()`（`@tauri-apps/api/app`）会因 ACL 被拦而静默失败，
   * 界面就退回到写死的旧号（表现得就是「版本号不对」）。
   */
  const [version, setVersion] = useState("");
  /** 自更新：检查/下载走 Rust 命令，最后一步用 JNI 拉系统安装器 */
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [dl, setDl] = useState<UpdateProgress | null>(null);
  /** 拉安装器失败时才把下载地址展示出来当退路 */
  const [installError, setInstallError] = useState<string | null>(null);

  useEffect(() => {
    api
      .appVersion()
      .then(setVersion)
      .catch(() => undefined);
  }, []);

  // 下载进度（Rust 端边写边发）
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<UpdateProgress>("update://progress", (ev) => setDl(ev.payload)).then((f) => {
      un = f;
    });
    return () => un?.();
  }, []);

  const pairing = settings.pairing;

  const probe = async () => {
    setBusy("probe");
    try {
      const info = await api.probeServer(base);
      onToast(`已连接 ${info.name} v${info.version}（已配对设备 ${info.paired ? "有" : "无"}）`);
    } catch (e) {
      onToast(`${e}`);
    } finally {
      setBusy(null);
    }
  };

  const pair = async () => {
    if (code.trim().length === 0) {
      onToast("请填写 PC 端显示的 6 位配对码");
      return;
    }
    setBusy("pair");
    try {
      const next = await api.pairServer(base, code.trim(), deviceName.trim() || "Android");
      onChange(next);
      setCode("");
      onToast("配对成功，正在全量同步…");
      await sync();
    } catch (e) {
      onToast(`${e}`);
    } finally {
      setBusy(null);
    }
  };

  const sync = async () => {
    setBusy("sync");
    setProgress(null);
    try {
      const report = await api.syncNow();
      onToast(
        `同步完成：新增 ${report.words} 词 / ${report.sentences} 句，耗时 ${(
          report.elapsed_ms / 1000
        ).toFixed(1)}s`,
      );
      const next = await api.getSettings();
      onChange(next);
    } catch (e) {
      onToast(`同步失败：${e}`);
    } finally {
      setBusy(null);
      setProgress(null);
    }
  };

  /**
   * 给 Promise 套一层前端超时。
   *
   * Rust 侧的请求自己是有超时的，但实测在「TCP 能连上、HTTPS 没响应」的网络下
   * 会一直不返回，界面上的按钮就无限转圈（用户只能杀进程）。
   * 所以前端再兜一层：超时就提示，至少能恢复交互。
   */
  const withTimeout = <T,>(promise: Promise<T>, ms: number, what: string) =>
    Promise.race([
      promise,
      new Promise<T>((_, reject) =>
        window.setTimeout(
          () => reject(new Error(`${what}超时（${ms / 1000} 秒），网络可能不通`)),
          ms,
        ),
      ),
    ]);

  const check = async () => {
    setChecking(true);
    try {
      const info = await withTimeout(api.checkUpdate(), 20000, "检查更新");
      setUpdate(info);
      if (!info) onToast("电脑端还没放新版本包");
      else if (!info.newer)
        onToast(
          `已是最新版本（v${info.version}${info.source === "online" ? "，来自网络" : "，来自电脑"}）`,
        );
    } catch (e) {
      onToast(`${e}`);
    } finally {
      setChecking(false);
    }
  };

  const doUpdate = async () => {
    setDownloading(true);
    setDl(null);
    setInstallError(null);

    // 第一步：下载（失败就是真没下下来）
    let path: string;
    try {
      path = await api.downloadUpdate();
    } catch (e) {
      onToast(`下载失败：${e}`);
      setDownloading(false);
      return;
    }

    // 第二步：JNI 调系统安装器（失败与下载无关，文案不混）
    try {
      await api.installUpdate(path);
      onToast("已交给系统安装器，请在弹窗里点「更新」");
    } catch (e) {
      setInstallError(String(e));
      onToast(`拉起安装器失败：${e}`);
    } finally {
      setDownloading(false);
    }
  };

  const unpair = async () => {
    setBusy("unpair");
    try {
      const next = await api.unpairServer();
      onChange(next);
      onToast("已解除配对");
    } catch (e) {
      onToast(`${e}`);
    } finally {
      setBusy(null);
    }
  };

  const setGoals = async (patch: Partial<Settings["goals"]>) => {
    const next = { ...settings, goals: { ...settings.goals, ...patch } };
    onChange(next);
    try {
      await api.saveSettings(next);
    } catch (e) {
      onToast(`保存失败：${e}`);
    }
  };

  const setAccent = async (accent: Accent) => {
    const next = { ...settings, audio_accent: accent };
    onChange(next);
    try {
      await api.saveSettings(next);
    } catch (e) {
      onToast(`保存失败：${e}`);
    }
  };

  const setTheme = async (theme: Theme) => {
    const next = { ...settings, theme };
    onChange(next);
    try {
      await api.saveSettings(next);
    } catch (e) {
      onToast(`保存失败：${e}`);
    }
  };

  /** 词卡位置：阅读页点词后卡片从哪边出来 */
  const setWordCardSide = async (side: WordCardSide) => {
    const next = { ...settings, word_card_side: side };
    onChange(next);
    try {
      await api.saveSettings(next);
    } catch (e) {
      onToast(`保存失败：${e}`);
    }
  };

  const reset = async () => {
    if (!window.confirm("会清空本机词库与学习进度（PC 端不受影响），确定吗？")) return;
    setBusy("reset");
    try {
      await api.resetAll();
      const next = await api.getSettings();
      onChange(next);
      onToast("已重置本机数据");
    } catch (e) {
      onToast(`重置失败：${e}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div>
      <div className="h1">设置</div>

      <div className="card">
        <div className="row between">
          <strong>手机同步</strong>
          <span className={`pill ${pairing ? "s3" : "s0"}`}>
            {pairing ? "已配对" : "未配对"}
          </span>
        </div>

        {pairing && (
          <div className="muted" style={{ marginTop: 8 }}>
            服务地址：{pairing.base}
            <br />
            设备：{pairing.server_name ?? "Pointra"} · {pairing.device_id.slice(0, 8)}
            {settings.last_sync_at ? (
              <>
                <br />
                上次同步：{new Date(settings.last_sync_at).toLocaleString()}
              </>
            ) : null}
          </div>
        )}

        <div style={{ height: 12 }} />

        <div className="field">
          <label>PC 地址（模拟器用 10.0.2.2:31415）</label>
          <input value={base} onChange={(e) => setBase(e.target.value)} placeholder="10.0.2.2:31415" />
        </div>

        {!pairing && (
          <>
            <div className="field">
              <label>6 位配对码</label>
              <input
                value={code}
                inputMode="numeric"
                maxLength={6}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, ""))}
                placeholder="123456"
              />
            </div>
            <div className="field">
              <label>本机名称</label>
              <input value={deviceName} onChange={(e) => setDeviceName(e.target.value)} />
            </div>
          </>
        )}

        <div className="row" style={{ flexWrap: "wrap" }}>
          <button className="btn ghost small" disabled={busy !== null} onClick={probe}>
            测试连接
          </button>
          {!pairing ? (
            <button className="btn small" disabled={busy !== null} onClick={pair}>
              配对
            </button>
          ) : (
            <>
              <button className="btn small" disabled={busy !== null} onClick={sync}>
                立即同步
              </button>
              <button className="btn danger small" disabled={busy !== null} onClick={unpair}>
                解除配对
              </button>
            </>
          )}
        </div>

        {busy === "sync" && (
          <div className="muted" style={{ marginTop: 10 }}>
            <span className="spinner" />
            正在同步… {progress ? `${progress.phase} ${progress.cursor}/${progress.target}` : ""}
          </div>
        )}
      </div>

      <div className="card">
        <div className="row between">
          <strong>应用更新</strong>
          <span className="muted">{version ? `当前 v${version}` : "当前版本未知"}</span>
        </div>

        {update && (
          <div style={{ marginTop: 8 }}>
            <div className="muted">
              {update.newer
                ? `发现新版本 v${update.version}（${(update.size / 1048576).toFixed(1)} MB）`
                : `最新版本 v${update.version}，已是最新`}
              {update.source === "online"
                ? " · 来自网络"
                : update.source === "pc"
                  ? " · 来自电脑"
                  : ""}
            </div>
            {/* 更新说明：发布时写进元数据的 notes，直接摊给用户看 */}
            {update.newer && update.notes && (
              <div className="update-notes">{update.notes}</div>
            )}
          </div>
        )}

        {downloading && dl && (
          <div className="muted" style={{ marginTop: 8 }}>
            正在下载 {(dl.downloaded / 1048576).toFixed(1)}
            {dl.total ? ` / ${(dl.total / 1048576).toFixed(1)}` : ""} MB
          </div>
        )}

        <div className="row" style={{ marginTop: 10, flexWrap: "wrap" }}>
          <button className="btn ghost small" disabled={checking || downloading} onClick={check}>
            {checking ? <span className="spinner" /> : "检查更新"}
          </button>
          {update?.newer && (
            <button className="btn small" disabled={downloading} onClick={doUpdate}>
              {downloading ? <span className="spinner" /> : "下载并安装"}
            </button>
          )}
        </div>

        {installError && settings.pairing && (
          <div className="muted" style={{ marginTop: 10, fontSize: 12 }}>
            退路：把下面地址粘到手机浏览器打开下载，下完点安装包即可覆盖更新：
            <br />
            <span className="link" style={{ userSelect: "text" }}>
              {settings.pairing.base.replace(/\/$/, "")}/api/app/download
            </span>
          </div>
        )}

        <div className="muted" style={{ marginTop: 8, fontSize: 12 }}>
          更新包由电脑端「手机同步」那个服务提供，需与电脑在同一 WiFi。
          安装时系统会问一次是否允许本应用安装应用，装完词库与学习进度都保留。
        </div>
      </div>

      <div className="card">
        <strong>每日上限</strong>
        <div className="field" style={{ marginTop: 12 }}>
          <label>新词 / 天</label>
          <input
            type="number"
            min={0}
            value={settings.goals.new_per_day}
            onChange={(e) => setGoals({ new_per_day: Number(e.target.value) || 0 })}
          />
        </div>
        <div className="field">
          <label>复习 / 天</label>
          <input
            type="number"
            min={0}
            value={settings.goals.review_per_day}
            onChange={(e) => setGoals({ review_per_day: Number(e.target.value) || 0 })}
          />
        </div>
        <div className="muted">修改后立即生效，无需重启。</div>
      </div>

      <div className="card">
        <strong>发音偏好</strong>
        <div className="chips" style={{ marginTop: 10 }}>
          <button className={settings.audio_accent === "us" ? "on" : ""} onClick={() => setAccent("us")}>
            美音
          </button>
          <button className={settings.audio_accent === "uk" ? "on" : ""} onClick={() => setAccent("uk")}>
            英音
          </button>
        </div>
      </div>

      <div className="card">
        <strong>阅读页词卡位置</strong>
        <div className="muted" style={{ margin: "6px 0 10px" }}>
          「自动」按你点的那一行在屏幕哪半边，把词卡放到另一边，尽量不遮住正在读的句子。
        </div>
        <div className="chips">
          <button
            className={cardSide === "auto" ? "on" : ""}
            onClick={() => void setWordCardSide("auto")}
          >
            自动
          </button>
          <button
            className={cardSide === "left" ? "on" : ""}
            onClick={() => void setWordCardSide("left")}
          >
            屏幕左侧
          </button>
          <button
            className={cardSide === "right" ? "on" : ""}
            onClick={() => void setWordCardSide("right")}
          >
            屏幕右侧
          </button>
          <button
            className={cardSide === "bottom" ? "on" : ""}
            onClick={() => void setWordCardSide("bottom")}
          >
            底部
          </button>
        </div>
      </div>

      <div className="card">
        <strong>主题</strong>
        <div className="chips" style={{ marginTop: 10 }}>
          <button
            className={settings.theme === "system" ? "on" : ""}
            onClick={() => setTheme("system")}
          >
            跟随系统
          </button>
          <button
            className={settings.theme === "light" ? "on" : ""}
            onClick={() => setTheme("light")}
          >
            浅色
          </button>
          <button
            className={settings.theme === "dark" ? "on" : ""}
            onClick={() => setTheme("dark")}
          >
            深色
          </button>
        </div>
      </div>

      <div className="card">
        <strong>危险操作</strong>
        <div className="muted" style={{ margin: "8px 0 12px" }}>
          清空本机词库、句子本与学习进度。PC 端数据不受影响，可重新同步回来。
        </div>
        <button className="btn danger" disabled={busy !== null} onClick={reset}>
          重置本机数据
        </button>
      </div>

      <div className="muted" style={{ textAlign: "center", padding: "4px 0 20px" }}>
        {version ? `Pointra Mobile v${version}` : "Pointra Mobile"}
      </div>
    </div>
  );
}
