import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { useI18n } from "../i18n";
import type { MessageKey } from "../i18n/messages";
import type { LauncherStatus } from "../types";

interface Props {
  status: LauncherStatus;
  notify: (msg: string) => void;
  onChanged: () => void;
}

type Operation =
  | { kind: "install" | "restart" }
  | { kind: "remove"; pkg: string };

type DoneState =
  | null
  | { kind: "ok" | "already" | "removed"; wasRunning: boolean }
  | { kind: "err"; operation: "install" | "remove" }
  | { kind: "notinstalled" };

/** Our own plugins, offered under the install button as one-click links.
 *  `pkg` is the npm package name (used by install idempotency + removal). */
const RECOMMENDED_PLUGINS: {
  label: MessageKey;
  desc: MessageKey;
  repo: string;
  pkg: string;
}[] = [
  {
    label: "plugins.rec_additive",
    desc: "plugins.rec_additive_desc",
    repo: "https://github.com/ConvFusion/DSH-additive",
    pkg: "dsh-additive",
  },
  {
    label: "plugins.rec_research",
    desc: "plugins.rec_research_desc",
    repo: "https://github.com/ConvFusion/ConvFusion-dsh",
    pkg: "dsh-convfusion",
  },
];

/**
 * Full install command for a recommended plugin repo.
 *
 * `dsh plugin` declares `--profile <name>` as a *required* option, and `web`
 * is the profile this launcher boots; without it the CLI exits with
 * `error: required option '--profile <name>' not specified`.
 */
const installCommand = (repo: string) =>
  `npx @deepseek-ai/dsh plugin --profile web add ${repo}`;

function formatElapsed(totalSeconds: number) {
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export default function Plugins({ status, notify, onChanged }: Props) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const [operation, setOperation] = useState<Operation | null>(null);
  const busy = operation !== null;
  const [elapsed, setElapsed] = useState(0);
  /** True once a log line indicates the downloader is retrying (slow network). */
  const [retryHint, setRetryHint] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [done, setDone] = useState<DoneState>(null);
  // Only offer a restart for a service that was running before the change
  // and is still running. A stopped service will load the changes on startup.
  const needsRestart = done !== null && "wasRunning" in done &&
    done.wasRunning && status.process.state === "running";
  /** Installed status of the two supported plugins, keyed by package name. */
  const [installed, setInstalled] = useState<Record<string, boolean>>({});
  const outRef = useRef<HTMLPreElement | null>(null);

  // Append output lines streamed by the backend (`dsh://plugin`); watch for
  // retry messages so the UI can reassure the user that work is still going.
  useEffect(() => {
    const un = listen<string>("dsh://plugin", (e) => {
      const line = e.payload;
      setLines((prev) => {
        const next = [...prev, line];
        return next.length > 300 ? next.slice(next.length - 300) : next;
      });
      if (/retry|retries?\s+left/i.test(line)) {
        setRetryHint(true);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // Elapsed-time ticker while busy: shows the job is alive even when the
  // log is quiet (e.g. during a slow GitHub download).
  useEffect(() => {
    if (!busy) return;
    setElapsed(0);
    const t0 = Date.now();
    const id = setInterval(() => setElapsed(Math.floor((Date.now() - t0) / 1000)), 1000);
    return () => clearInterval(id);
  }, [busy]);

  // Keep the log scrolled to the bottom.
  useEffect(() => {
    const el = outRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines]);

  // Fetch installed status for the two supported plugins.
  const refreshInstalled = useCallback(async () => {
    try {
      const list = await api.pluginStatus();
      setInstalled(Object.fromEntries(list.map((p) => [p.name, p.installed])));
    } catch {
      /* non-fatal — the profile may not exist yet */
    }
  }, []);

  useEffect(() => {
    refreshInstalled();
  }, [refreshInstalled]);

  // Preview of the command that will be executed: full `npx …` input is
  // run as-is, anything else is wrapped into the standard plugin command
  // (mirrors the backend's plugin_npx_args).
  const trimmed = name.trim();
  const willRun = trimmed
    ? trimmed === "npx" || trimmed.startsWith("npx ")
      ? trimmed
      : `npx -y --package @deepseek-ai/dsh dsh plugin --profile web add ${trimmed}`
    : "";

  function resetResult() {
    setLines([]);
    setDone(null);
    setRetryHint(false);
  }

  async function install() {
    if (!trimmed || busy) return;
    const wasRunning = status.process.state === "running";
    setOperation({ kind: "install" });
    resetResult();
    try {
      const res = await api.installPlugin(trimmed);
      if (res === "already-installed") {
        setDone({ kind: "already", wasRunning });
      } else {
        setDone({ kind: "ok", wasRunning });
      }
      await refreshInstalled();
      onChanged();
    } catch (e) {
      setDone({ kind: "err", operation: "install" });
      notify(String(e));
    } finally {
      setOperation(null);
    }
  }

  // One-click: fill the input with the full install command for one of our
  // own plugins, so the user only has to press Install.
  function fillRecommended(repo: string) {
    if (busy) return;
    setName(installCommand(repo));
    if (done) setDone(null);
  }

  // Remove one of the launcher's own plugins from the web profile.
  async function removePlugin(pkg: string) {
    if (busy) return;
    const wasRunning = status.process.state === "running";
    setOperation({ kind: "remove", pkg });
    resetResult();
    try {
      const res = await api.removePlugin(pkg);
      if (res === "removed") {
        setDone({ kind: "removed", wasRunning });
      } else {
        setDone({ kind: "notinstalled" });
      }
      await refreshInstalled();
      onChanged();
    } catch (e) {
      setDone({ kind: "err", operation: "remove" });
      setLines((prev) => [...prev, String(e)]);
    } finally {
      setOperation(null);
    }
  }

  async function restartNow() {
    if (busy || !needsRestart) return;
    setOperation({ kind: "restart" });
    resetResult();
    try {
      await api.restartDsh(false);
      onChanged();
      notify(t("plugins.restarted"));
    } catch (e) {
      notify(String(e));
    } finally {
      setOperation(null);
    }
  }

  return (
    <div className="settings plugins">
      <div className="card section">
        <h3>{t("plugins.title")}</h3>
        <p className="desc">{t("plugins.desc")}</p>
        <ul className="plugin-desc-list">
          <li>{t("plugins.desc_npm")}</li>
          <li>{t("plugins.desc_github")}</li>
          <li>{t("plugins.desc_path")}</li>
          <li>{t("plugins.desc_npx")}</li>
        </ul>
        {status.process.state === "running" && (
          <p className="desc">{t("plugins.desc_restart")}</p>
        )}

        <div className="plugin-input">
          <input
            className="plugin-name"
            type="text"
            value={name}
            placeholder={t("plugins.placeholder")}
            spellCheck={false}
            autoComplete="off"
            disabled={busy}
            onChange={(e) => {
              setName(e.target.value);
              if (done) setDone(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") install();
            }}
          />
          <button
            className="btn primary"
            disabled={busy || !name.trim()}
            onClick={install}
          >
            {operation?.kind === "install" && <span className="spinner" />}
            {t("plugins.install")}
          </button>
        </div>

        {/* Recommended plugins: one-click fill of the full install command. */}
        <div className="plugin-recommended">
          <div className="recommended-title">{t("plugins.rec_title")}</div>
          <ul className="recommended-list">
            {RECOMMENDED_PLUGINS.map((p) => (
              <li key={p.repo} className="recommended-item">
                <button
                  type="button"
                  className="recommended-card"
                  disabled={busy}
                  title={installCommand(p.repo)}
                  onClick={() => fillRecommended(p.repo)}
                >
                  <span className="recommended-name">
                    {t(p.label)}
                    <span className="recommended-desc"> - {t(p.desc)}</span>
                  </span>
                  <span className="recommended-repo">{p.repo}</span>
                </button>
                {/* Installed badge + remove button for our own plugins. */}
                {installed[p.pkg] && (
                  <div className="plugin-installed-row">
                    <span className="installed-badge">✓ {t("plugins.installed")}</span>
                    <button
                      className="btn small remove"
                      disabled={busy}
                      onClick={() => removePlugin(p.pkg)}
                    >
                      {operation?.kind === "remove" && operation.pkg === p.pkg ? (
                        <><span className="spinner" />{t("plugins.removing")}</>
                      ) : t("plugins.remove")}
                    </button>
                  </div>
                )}
              </li>
            ))}
          </ul>
          <div className="recommended-hint">{t("plugins.rec_hint")}</div>
        </div>

        {willRun && !busy && !done && (
          <div className="plugin-willrun">
            <span className="willrun-label">{t("plugins.will_run")}</span>
            <code>{willRun}</code>
          </div>
        )}

        {/* Progress belongs to the active operation, not always an install. */}
        {operation && (
          <div className={`plugin-banner busy${operation.kind === "remove" ? " removing" : ""}`}>
            <span className="spinner" />
            <div>
              <div className="banner-title">
                {t(operation.kind === "remove" ? "plugins.removing_title" :
                  operation.kind === "restart" ? "plugins.restarting" : "plugins.installing")}
                <span className="elapsed">{formatElapsed(elapsed)}</span>
              </div>
              {operation.kind !== "restart" && (
                <div className="banner-sub">
                  {t(operation.kind === "remove" ? "plugins.removing_hint" : "plugins.installing_hint")}
                </div>
              )}
            </div>
          </div>
        )}
        {operation?.kind === "install" && retryHint && (
          <div className="plugin-banner warn">⚠ {t("plugins.retry_hint")}</div>
        )}

        {done?.kind === "ok" && (
          <div className="plugin-banner ok">
            <div>
              <div className="banner-title">{t("plugins.restart_hint_title")}</div>
              {needsRestart && <div className="banner-sub">{t("plugins.restart_hint_sub")}</div>}
            </div>
            {needsRestart && (
              <button className="btn small" disabled={busy} onClick={restartNow}>
                ↻ {t("plugins.restart_now")}
              </button>
            )}
          </div>
        )}
        {done?.kind === "already" && (
          <div className="plugin-banner ok">
            <div>
              <div className="banner-title">{t("plugins.already_title")}</div>
              <div className="banner-sub">{t("plugins.already_sub")}</div>
            </div>
            {needsRestart && (
              <button className="btn small" disabled={busy} onClick={restartNow}>
                ↻ {t("plugins.restart_now")}
              </button>
            )}
          </div>
        )}
        {done?.kind === "removed" && (
          <div className="plugin-banner removed">
            <div>
              <div className="banner-title">{t("plugins.removed_title")}</div>
              {needsRestart && <div className="banner-sub">{t("plugins.removed_sub")}</div>}
            </div>
            {needsRestart && (
              <button className="btn small" disabled={busy} onClick={restartNow}>
                ↻ {t("plugins.restart_now")}
              </button>
            )}
          </div>
        )}
        {done?.kind === "notinstalled" && (
          <div className="plugin-banner removed">{t("plugins.not_installed")}</div>
        )}
        {done?.kind === "err" && (
          <div className="plugin-banner err">
            {t(done.operation === "remove" ? "plugins.remove_failed" : "plugins.install_failed")}
          </div>
        )}

        {(busy || lines.length > 0) && (
          <pre className="plugin-output" ref={outRef}>
            {lines.length > 0 ? lines.join("\n") : t("plugins.waiting")}
          </pre>
        )}
      </div>
    </div>
  );
}
