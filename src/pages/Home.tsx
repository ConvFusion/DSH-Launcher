import { useRef, useState } from "react";
import type {
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
  onMainAction: () => void;
  onUpdate: () => void;
  onStart: () => void;
  onStop: () => void;
  onRestart: () => void;
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
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isBusy = busyPhase !== null;
  const isInstalled = env.ready; // Node + DSH both present
  const isRunning = procState === "running";
  const isExternal = status.process.external;

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
      {isBusy && envProgress?.message && (
        <p className="progress">{envProgress.message}</p>
      )}
    </div>
  );
}
