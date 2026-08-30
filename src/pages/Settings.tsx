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
  const theme = cfg.theme ?? "system";

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

      {/* About */}
      <div className="card section">
        <h3>{t("settings.about")}</h3>
        <div className="setting-row" style={{ paddingTop: 0 }}>
          <span className="label">DSH Launcher</span>
          <span className="v">v{status.launcher_version}</span>
        </div>
        <p className="desc" style={{ marginBottom: 0 }}>
          {t("settings.about_desc")}
        </p>
      </div>
    </div>
  );
}
