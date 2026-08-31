import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { useI18n } from "../i18n";

interface Props {
  notify: (msg: string) => void;
  onChanged: () => void;
}

type DoneState = null | "ok" | "err";

function formatElapsed(totalSeconds: number) {
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export default function Plugins({ notify, onChanged }: Props) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  /** True once a log line indicates the downloader is retrying (slow network). */
  const [retryHint, setRetryHint] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [done, setDone] = useState<DoneState>(null);
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
    setBusy(true);
    resetResult();
    try {
      await api.installPlugin(trimmed);
      setDone("ok");
      onChanged();
    } catch (e) {
      setDone("err");
      notify(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function restartNow() {
    if (busy) return;
    setBusy(true);
    resetResult();
    try {
      await api.restartDsh(false);
      onChanged();
      notify(t("plugins.restarted"));
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(false);
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
        <p className="desc">{t("plugins.desc_restart")}</p>

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
            {busy && <span className="spinner" />}
            {t("plugins.install")}
          </button>
        </div>

        {willRun && !busy && (
          <div className="plugin-willrun">
            <span className="willrun-label">{t("plugins.will_run")}</span>
            <code>{willRun}</code>
          </div>
        )}

        {/* Busy: reassure the user the install is alive and may be slow. */}
        {busy && (
          <div className="plugin-banner busy">
            <span className="spinner" />
            <div>
              <div className="banner-title">
                {t("plugins.installing")}
                <span className="elapsed">{formatElapsed(elapsed)}</span>
              </div>
              <div className="banner-sub">{t("plugins.installing_hint")}</div>
            </div>
          </div>
        )}
        {busy && retryHint && (
          <div className="plugin-banner warn">⚠ {t("plugins.retry_hint")}</div>
        )}

        {done === "ok" && (
          <div className="plugin-banner ok">
            <div>
              <div className="banner-title">{t("plugins.restart_hint_title")}</div>
              <div className="banner-sub">{t("plugins.restart_hint_sub")}</div>
            </div>
            <button className="btn small" disabled={busy} onClick={restartNow}>
              ↻ {t("plugins.restart_now")}
            </button>
          </div>
        )}
        {done === "err" && (
          <div className="plugin-banner err">{t("plugins.install_failed")}</div>
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
