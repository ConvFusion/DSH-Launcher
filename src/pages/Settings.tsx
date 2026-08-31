import { useState } from "react";
import type { LauncherStatus } from "../types";
import { api } from "../api";
import { useI18n } from "../i18n";

interface Props {
  status: LauncherStatus;
  notify: (msg: string) => void;
  onChanged: () => void;
}

export default function Settings({ status, notify, onChanged }: Props) {
  const cfg = status.config;
  const { t, lang, setLang } = useI18n();
  const [busy, setBusy] = useState<string | null>(null);
  const [nodePath, setNodePath] = useState(cfg.node_path ?? "");
  const theme = cfg.theme ?? "system";

  async function saveNode() {
    setBusy("node");
    try {
      await api.updateConfig({ node_path: nodePath.trim() });
      onChanged();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function clearNode() {
    setBusy("node");
    setNodePath("");
    try {
      await api.updateConfig({ node_path: "" });
      onChanged();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function copyDiagnostics() {
    setBusy("diagnose");
    try {
      const lines = await api.diagnoseEnvironment();
      await navigator.clipboard.writeText(lines.join("\n"));
      notify(t("settings.diagnose_copied"));
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function changeLang(next: "en" | "zh") {
    setBusy("language");
    setLang(next);
    try {
      await api.updateConfig({ language: next });
      onChanged();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function changeTheme(next: "system" | "light" | "dark") {
    setBusy("theme");
    try {
      await api.updateConfig({ theme: next });
      onChanged();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="settings">
      {/* Language */}
      <div className="card section">
        <h3>{t("settings.language")}</h3>
        <p className="desc">{t("settings.language_desc")}</p>
        {(["en", "zh"] as const).map((l) => (
          <div className="setting-row" key={l}>
            <div>
              <div className="label">{t(`settings.language.${l}`)}</div>
              <div className="hint">
                {l === "en" ? "English" : "简体中文"}
              </div>
            </div>
            <input
              type="radio"
              name="lang"
              className="lang-radio"
              checked={lang === l}
              disabled={busy === "language"}
              onChange={() => changeLang(l)}
            />
          </div>
        ))}
      </div>

      {/* Theme */}
      <div className="card section">
        <h3>{t("settings.theme")}</h3>
        <p className="desc">{t("settings.theme_desc")}</p>
        {(["system", "light", "dark"] as const).map((th) => (
          <div className="setting-row" key={th}>
            <div>
              <div className="label">{t(`settings.theme.${th}`)}</div>
              <div className="hint">
                {th === "system" ? "auto" : th === "light" ? "☀" : "☾"}
              </div>
            </div>
            <input
              type="radio"
              name="theme"
              className="lang-radio"
              checked={theme === th}
              disabled={busy === "theme"}
              onChange={() => changeTheme(th)}
            />
          </div>
        ))}
      </div>

      {/* Node.js runtime override */}
      <div className="card section">
        <h3>{t("settings.node")}</h3>
        <p className="desc">{t("settings.node_desc")}</p>
        <div className="setting-row" style={{ paddingTop: 0 }}>
          <input
            type="text"
            className="node-path-input"
            placeholder={t("settings.node_placeholder")}
            value={nodePath}
            disabled={busy === "node"}
            onChange={(e) => setNodePath(e.target.value)}
            spellCheck={false}
          />
        </div>
        <div className="setting-row" style={{ justifyContent: "flex-end", gap: "8px" }}>
          <button
            className="btn secondary"
            onClick={saveNode}
            disabled={busy === "node"}
          >
            {t("settings.node_save")}
          </button>
          {cfg.node_path ? (
            <button
              className="btn secondary"
              onClick={clearNode}
              disabled={busy === "node"}
            >
              {t("settings.node_clear")}
            </button>
          ) : null}
        </div>
      </div>

      {/* About + Diagnostics */}
      <div className="card section">
        <h3>{t("settings.about")}</h3>
        <div className="setting-row" style={{ paddingTop: 0 }}>
          <span className="label">DSH Launcher</span>
          <span className="v">v{status.launcher_version}</span>
        </div>
        <p className="desc">{t("settings.about_desc")}</p>
        <div className="setting-row">
          <div>
            <div className="label">{t("settings.diagnose")}</div>
            <div className="hint">{t("settings.diagnose_desc")}</div>
          </div>
          <button
            className="btn secondary"
            onClick={copyDiagnostics}
            disabled={busy === "diagnose"}
          >
            {t("settings.diagnose_copy")}
          </button>
        </div>
      </div>
    </div>
  );
}
