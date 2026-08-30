import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  EnvProgress,
  LauncherStatus,
  MainPhase,
  UpdateInfo,
} from "./types";
import { api } from "./api";
import Logo from "./components/Logo";
import Home from "./pages/Home";
import Settings from "./pages/Settings";
import { I18nProvider, initialLanguage, useI18n } from "./i18n";
import type { Language } from "./i18n/messages";
import "./styles.css";

type View = "home" | "settings";

export default function App() {
  const [status, setStatus] = useState<LauncherStatus | null>(null);
  const [envProgress, setEnvProgress] = useState<EnvProgress | null>(null);
  const [busyPhase, setBusyPhase] = useState<MainPhase | null>(null);
  const [view, setView] = useState<View>("home");
  const [toast, setToast] = useState<string | null>(null);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const notify = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) clearTimeout(toastTimer.current);
    toastTimer.current = setTimeout(() => setToast(null), 4200);
  }, []);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.getStatus());
    } catch {
      /* backend not ready yet */
    }
  }, []);

  useEffect(() => {
    refresh();
    const un1 = listen<LauncherStatus>("dsh://status", (e) => setStatus(e.payload));
    const un2 = listen<EnvProgress>("dsh://env", (e) => setEnvProgress(e.payload));
    // The browser-chooser flow is gone: navigate events (if any) are ignored.
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, [refresh]);

  // Latest published version + whether an update is available — detected
  // from the npm registry (the version `npx @deepseek-ai/dsh` would install).
  const checkUpdate = useCallback(() => {
    api
      .checkDshUpdate()
      .then(setUpdateInfo)
      .catch(() => {});
  }, []);

  useEffect(() => {
    checkUpdate();
  }, [checkUpdate]);

  // Language lives in the backend config (survives restarts); when the
  // status arrives we derive the provider's language from it.
  const lang = initialLanguage(status?.config.language ?? null);

  // Apply the persisted theme to <html data-theme="...">. "system" (default)
  // leaves the attribute unset so the CSS @media (prefers-color-scheme) rule
  // follows the OS automatically.
  useEffect(() => {
    const theme = status?.config.theme ?? "system";
    const root = document.documentElement;
    if (theme === "system") {
      root.removeAttribute("data-theme");
    } else {
      root.setAttribute("data-theme", theme);
    }
  }, [status?.config.theme]);

  const handleLangChange = useCallback(async (next: Language) => {
    try {
      await api.updateConfig({ language: next });
    } catch {
      /* best effort — the provider still switches locally */
    }
  }, []);

  return (
    <I18nProvider lang={lang} onLangChange={handleLangChange}>
      <AppShell
        status={status}
        envProgress={envProgress}
        busyPhase={busyPhase}
        setBusyPhase={setBusyPhase}
        view={view}
        setView={setView}
        toast={toast}
        updateInfo={updateInfo}
        checkUpdate={checkUpdate}
        refresh={refresh}
        notify={notify}
      />
    </I18nProvider>
  );
}

function AppShell({
  status,
  envProgress,
  busyPhase,
  setBusyPhase,
  view,
  setView,
  toast,
  updateInfo,
  checkUpdate,
  refresh,
  notify,
}: {
  status: LauncherStatus | null;
  envProgress: EnvProgress | null;
  busyPhase: MainPhase | null;
  setBusyPhase: (p: MainPhase | null) => void;
  view: View;
  setView: (v: View) => void;
  toast: string | null;
  updateInfo: UpdateInfo | null;
  checkUpdate: () => void;
  refresh: () => Promise<void>;
  notify: (msg: string) => void;
}) {
  const { t } = useI18n();

  if (!status) {
    return (
      <div
        style={{
          height: "100%",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 14,
          color: "var(--muted)",
        }}
      >
        <Logo size={56} />
        <span className="spinner" />
        <span style={{ fontSize: 12.5 }}>{t("app.loading")}</span>
      </div>
    );
  }

  // The same "can open" decision drives both the button label (Home) and
  // what a click does:
  //  * service running OR installed → open (starts the service first if
  //    needed, then opens the browser — never installs)
  //  * not installed and not running → install everything (Node.js first,
  //    then DSH), start, open
  const currentStatus = status; // narrowed — safe to capture in closures
  const canOpen =
    currentStatus.process.state === "running" || currentStatus.env.ready;

  async function mainAction() {
    try {
      if (canOpen) {
        setBusyPhase("opening");
        await api.openHarness();
      } else {
        setBusyPhase("installing");
        const report = await api.ensureEnvironment();
        if (report.error) throw new Error(report.error);
        await refresh();
        setBusyPhase("opening");
        await api.openHarness();
      }
      await refresh();
      checkUpdate();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusyPhase(null);
    }
  }

  // Update: stop the running service first (Windows locks files in use),
  // install the latest @deepseek-ai/dsh from npm — the same package that
  // `npx @deepseek-ai/dsh web` fetches — then start it again.
  async function updateDsh() {
    const wasRunning = currentStatus.process.state === "running";
    setBusyPhase("updating");
    try {
      if (wasRunning) await api.stopDsh();
      const version = await api.installDsh();
      await refresh();
      if (wasRunning) await api.restartDsh(false);
      await refresh();
      checkUpdate();
      notify(t("home.updated", { version }));
    } catch (e) {
      notify(String(e));
    } finally {
      setBusyPhase(null);
    }
  }

  const inSettings = view === "settings";

  return (
    <div className="app">
      <button
        className="gear"
        aria-label={inSettings ? t("app.close_settings") : t("app.settings")}
        title={inSettings ? t("app.close_settings") : t("app.settings")}
        onClick={() => setView(inSettings ? "home" : "settings")}
      >
        {inSettings ? "←" : "⚙︎"}
      </button>

      <div className="main">
        {inSettings ? (
          <Settings status={status} notify={notify} onChanged={refresh} />
        ) : (
          <Home
            status={status}
            canOpen={canOpen}
            busyPhase={busyPhase}
            envProgress={envProgress}
            updateInfo={updateInfo}
            onMainAction={mainAction}
            onUpdate={updateDsh}
          />
        )}
      </div>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
