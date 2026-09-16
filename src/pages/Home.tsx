import { useEffect, useRef, useState } from "react";
import type {
  ConsoleKind,
  EnvProgress,
  LauncherStatus,
  MainPhase,
  UpdateInfo,
} from "../types";
import Logo from "../components/Logo";
import { useI18n } from "../i18n";

interface Props {
  status: LauncherStatus;
  /**
   * Whether the main button opens the harness: the service is running
   * (even if our package detection can't see the install, e.g. DSH started
   * manually with `npx @deepseek-ai/dsh web`) or it is installed.
   */
  canOpen: boolean;
  /** What action is in progress (null = idle). */
  busyPhase: MainPhase | null;
  envProgress: EnvProgress | null;
  /** npm registry info: latest version + whether an update is available. */
  updateInfo: UpdateInfo | null;
  /** Live console log of the install/update command (`dsh://update`). */
  updateLines: string[];
  /** Set when the last install/update attempt failed; keeps the log on screen. */
  updateError: string | null;
  /** Which command the log belongs to, so the panel is titled correctly. */
  consoleKind: ConsoleKind | null;
  onDismissUpdateLog: () => void;
  onMainAction: () => void;
  onUpdate: () => void;
  onStart: () => void;
  onStop: () => void;
  onRestart: () => void;
}

/** "m:ss" elapsed-time label, so a quiet log still looks alive. */
function formatElapsed(totalSeconds: number) {
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// canOpen is computed in App and drives the main action (install vs open);
// we accept it as a prop even though the rendering logic uses isInstalled/isRunning
// directly — kept for backward compatibility with App's mainAction closure.
export default function Home({
  status,
  canOpen: _canOpen,
  busyPhase,
  envProgress,
  updateInfo,
  updateLines,
  updateError,
  consoleKind,
  onDismissUpdateLog,
  onMainAction,
  onUpdate,
  onStart,
  onStop,
  onRestart,
}: Props) {
  const { t } = useI18n();
  const env = status.env;
  const procState = status.process.state;
  const [copied, setCopied] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const logRef = useRef<HTMLPreElement | null>(null);

  const isBusy = busyPhase !== null;
  const isInstalled = env.ready; // Node + DSH both present
  const isRunning = procState === "running";
  const isExternal = status.process.external;

  const isUpdating = busyPhase === "updating";
  const isInstalling = busyPhase === "installing";
  // A first install runs the same kind of long npm command as an update, so it
  // gets the same live console panel.
  const consoleBusy = isUpdating || isInstalling;
  // The log panel lives on while the command runs, and stays after a failure so
  // the npm output that explains it is still readable.
  const showUpdateLog = consoleBusy || (updateError !== null && updateLines.length > 0);
  const logTitle = updateError
    ? consoleKind === "install"
      ? t("home.install_failed")
      : t("home.update_failed")
    : consoleKind === "install"
      ? t("home.install_log")
      : t("home.update_log");

  // Elapsed-time ticker: npm can be silent for long stretches on a slow
  // network, and a frozen log otherwise looks like a hang.
  useEffect(() => {
    if (!consoleBusy) return;
    setElapsed(0);
    const t0 = Date.now();
    const id = setInterval(() => setElapsed(Math.floor((Date.now() - t0) / 1000)), 1000);
    return () => clearInterval(id);
  }, [consoleBusy]);

  // Follow the tail of the log as lines stream in.
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [updateLines]);

  // ---- Main button label ----
  let mainLabel: string;
  if (busyPhase) {
    mainLabel = t(`home.busy.${busyPhase}`);
  } else if (!isInstalled && !isRunning) {
    mainLabel = updateInfo?.latest
      ? t("home.install_v", { version: updateInfo.latest })
      : t("home.install");
  } else {
    const version = env.dsh?.version ?? updateInfo?.latest ?? null;
    mainLabel = version ? t("home.open_v", { version }) : t("home.open");
  }

  // Update only offered for installs we manage.
  const showUpdate =
    isInstalled && !isBusy && updateInfo?.update_available === true;

  // ---- When are control buttons shown? ----
  // * Only when DSH is installed (not before install).
  // * Show "Start"  when stopped/error.
  // * Show "Stop"/"Restart" when running.
  // * Starting/stopping: all controls disabled, spinner on main button.
  // * External instance (started outside launcher): show Restart/Stop but
  //   note that stopping an external may fail.
  const showControls = isInstalled || isRunning;
  const showStart = showControls && !isBusy && (procState === "stopped" || procState === "error");
  const showStop = showControls && !isBusy && isRunning;
  const showRestart = showControls && !isBusy && isRunning;

  // Main button disabled when busy. When DSH is already running the main
  // button is "Open" (opens browser) and stays clickable even when other
  // controls exist.
  const mainDisabled =
    isBusy ||
    (procState === "starting" || procState === "stopping");

  async function copyUrl() {
    const url = status.process.url;
    try {
      await navigator.clipboard.writeText(url);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = url;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        /* ignore */
      }
      document.body.removeChild(ta);
    }
    setCopied(true);
    if (copyTimer.current) clearTimeout(copyTimer.current);
    copyTimer.current = setTimeout(() => setCopied(false), 2000);
  }

  return (
    <div className="home">
      <Logo size={104} />

      {/* Status line with colored dot */}
      <div className={`status-dot status-${procState}`}>
        <span className="dot" />
        <span>{t(`status.${procState}`)}</span>
        {isExternal && !isBusy && (
          <span className="status-hint">（外部实例）</span>
        )}
      </div>

      {/* Primary big button */}
      <button
        className="btn big primary"
        disabled={mainDisabled}
        onClick={onMainAction}
      >
        {(busyPhase || procState === "starting" || procState === "stopping") && (
          <span className="spinner" />
        )}
        {mainLabel}
      </button>

      {/* Secondary control row: Start / Stop / Restart */}
      {showControls && (
        <div className="controls">
          {showStart && (
            <button
              className="btn ctrl"
              onClick={onStart}
              title="启动 DeepSeek Harness 服务"
            >
              ▶ {t("home.start")}
            </button>
          )}
          {showRestart && (
            <button
              className="btn ctrl"
              onClick={onRestart}
              title="重启 DeepSeek Harness 服务"
            >
              ↻ {t("home.restart")}
            </button>
          )}
          {showStop && (
            <button
              className="btn ctrl danger"
              onClick={onStop}
              title="停止 DeepSeek Harness 服务"
            >
              ■ {t("home.stop")}
            </button>
          )}
        </div>
      )}

      {/* Update button */}
      {showUpdate && (
        <button
          className="btn small update"
          onClick={onUpdate}
        >
          {updateInfo.latest
            ? t("home.update_v", { version: updateInfo.latest })
            : t("home.update")}
        </button>
      )}

      {/* URL (only when running) */}
      {isRunning && (
        <button
          className="url-copy"
          onClick={copyUrl}
          title={t("home.copy_url")}
        >
          {status.process.url}
          {copied && <span className="copied-ok">✓</span>}
        </button>
      )}

      {/* Error / progress message */}
      {procState === "error" && status.process.error && !isBusy && (
        <p className="progress error">{status.process.error}</p>
      )}
      {isBusy && !isUpdating && envProgress?.message && (
        <p className="progress">{envProgress.message}</p>
      )}

      {/* Live console log of the install/update command. */}
      {showUpdateLog && (
        <div className="update-log">
          <div className="update-log-head">
            {consoleBusy && <span className="spinner" />}
            <span className="update-log-title">{logTitle}</span>
            {consoleBusy && <span className="elapsed">{formatElapsed(elapsed)}</span>}
            {!consoleBusy && (
              <button
                className="update-log-dismiss"
                onClick={onDismissUpdateLog}
                title={t("home.update_log_dismiss")}
                aria-label={t("home.update_log_dismiss")}
              >
                ✕
              </button>
            )}
          </div>
          {updateError && <p className="progress error">{updateError}</p>}
          <pre className="plugin-output" ref={logRef}>
            {updateLines.length > 0
              ? updateLines.join("\n")
              : t("home.update_waiting")}
          </pre>
        </div>
      )}

      {/* The launcher's own version, at the foot of the home column — bug
          reports and support requests should be able to name the exact build.
          Selectable so it can be copied. */}
      <div className="launcher-version">
        {t("home.launcher_version", { version: status.launcher_version })}
      </div>
    </div>
  );
}
