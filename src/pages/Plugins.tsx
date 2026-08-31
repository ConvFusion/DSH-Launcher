import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { useI18n } from "../i18n";

interface Props {
  notify: (msg: string) => void;
  onChanged: () => void;
}

type DoneState = null | "ok" | "err";

export default function Plugins({ notify, onChanged }: Props) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [done, setDone] = useState<DoneState>(null);
  const outRef = useRef<HTMLPreElement | null>(null);

  // Append output lines streamed by the backend (`dsh://plugin`).
  useEffect(() => {
    const un = listen<string>("dsh://plugin", (e) => {
      setLines((prev) => {
        const next = [...prev, e.payload];
        return next.length > 300 ? next.slice(next.length - 300) : next;
      });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // Keep the log scrolled to the bottom.
  useEffect(() => {
    const el = outRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines]);

  function resetResult() {
    setLines([]);
    setDone(null);
  }

  async function install() {
    const trimmed = name.trim();
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

        {lines.length > 0 && (
          <pre className="plugin-output" ref={outRef}>
            {lines.join("\n")}
          </pre>
        )}
      </div>
    </div>
  );
}
