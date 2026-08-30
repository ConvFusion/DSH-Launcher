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
   * manually with `npx @deepseek-ai/dsh web`) or it is installed. Computed
   * in App — the same value drives the button label and the click action.
   */
  canOpen: boolean;
  /** What the main button is doing right now (null = idle). */
  busyPhase: MainPhase | null;
  envProgress: EnvProgress | null;
  /** npm registry info: latest version + whether an update is available. */
  updateInfo: UpdateInfo | null;
  onMainAction: () => void;
  onUpdate: () => void;
}

export default function Home({
  status,
  canOpen,
  busyPhase,
  envProgress,
  updateInfo,
  onMainAction,
  onUpdate,
}: Props) {
  const { t } = useI18n();
  const env = status.env;
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // One button, two states:
  //  * service running or installed → "Open DeepSeek Harness (v…)"
  //  * not installed               → "Install DeepSeek Harness (v…)"
  let label: string;
  if (busyPhase) {
    label = t(`home.busy.${busyPhase}`);
  } else if (canOpen) {
    const version = env.dsh?.version ?? updateInfo?.latest ?? null;
    label = version ? t("home.open_v", { version }) : t("home.open");
  } else {
    label = updateInfo?.latest
      ? t("home.install_v", { version: updateInfo.latest })
      : t("home.install");
  }

  // Update is only offered for installs we manage (env.dsh known) — never
  // for an externally started instance we cannot update.
  const showUpdate =
    canOpen && env.ready && updateInfo?.update_available === true;

  async function copyUrl() {
    const url = status.process.url;
    try {
      await navigator.clipboard.writeText(url);
    } catch {
      // Fallback for webviews without the async clipboard API.
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
      <button
        className="btn big primary"
        disabled={busyPhase !== null}
        onClick={onMainAction}
      >
        {busyPhase !== null && <span className="spinner" />}
        {label}
      </button>
      {showUpdate && (
        <button
          className="btn small update"
          disabled={busyPhase !== null}
          onClick={onUpdate}
        >
          {busyPhase === "updating" && <span className="spinner" />}
          {updateInfo.latest
            ? t("home.update_v", { version: updateInfo.latest })
            : t("home.update")}
        </button>
      )}
      {canOpen && (
        <button
          className="url-copy"
          onClick={copyUrl}
          title={t("home.copy_url")}
        >
          {status.process.url}
          {copied && <span className="copied-ok">✓</span>}
        </button>
      )}
      {busyPhase !== null && envProgress?.message && (
        <p className="progress">{envProgress.message}</p>
      )}
    </div>
  );
}
