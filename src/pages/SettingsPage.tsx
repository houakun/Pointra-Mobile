import { useState } from "react";
import { api } from "../api";
import type { Accent, Settings, SyncProgress } from "../types";

interface Props {
  settings: Settings;
  onChange: (s: Settings) => void;
  onToast: (msg: string) => void;
}

export default function SettingsPage({ settings, onChange, onToast }: Props) {
  const [base, setBase] = useState(settings.pairing?.base ?? "10.0.2.2:31415");
  const [code, setCode] = useState("");
  const [deviceName, setDeviceName] = useState("Android");
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<SyncProgress | null>(null);

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
        <strong>危险操作</strong>
        <div className="muted" style={{ margin: "8px 0 12px" }}>
          清空本机词库、句子本与学习进度。PC 端数据不受影响，可重新同步回来。
        </div>
        <button className="btn danger" disabled={busy !== null} onClick={reset}>
          重置本机数据
        </button>
      </div>

      <div className="muted" style={{ textAlign: "center", padding: "4px 0 20px" }}>
        Pointra Mobile v0.1.0
      </div>
    </div>
  );
}
