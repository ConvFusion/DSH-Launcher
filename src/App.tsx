import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  ConsoleKind,
  EnvProgress,
  LauncherStatus,
  MainPhase,
  UpdateInfo,
} from "./types";
import { api } from "./api";
import Logo from "./components/Logo";
import { PuzzleIcon, SettingsIcon, RefreshIcon } from "./components/Icons";
import Home from "./pages/Home";
import Settings from "./pages/Settings";
import Plugins from "./pages/Plugins";
import { I18nProvider, initialLanguage, useI18n } from "./i18n";
import type { Language } from "./i18n/messages";
import "./styles.css";

type View = "home" | "settings" | "plugins";

export default function App() {
  const [status, setStatus] = useState<LauncherStatus | null>(null);
  const [envProgress, setEnvProgress] = useState<EnvProgress | null>(null);
  const [busyPhase, setBusyPhase] = useState<MainPhase | null>(null);
  const [view, setView] = useState<View>("home");
  const [toast, setToast] = useState<string | null>(null);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [launcherUpdateInfo, setLauncherUpdateInfo] = useState<UpdateInfo | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  /**
   * Live console log of the running DSH install/update command
   * (`dsh://update`), shown on the home page while the button reads
   * "Installing…"/"Updating…" — and kept on screen if it failed, so the
   * reason stays readable after the toast disappears.
   */
  const [updateLines, setUpdateLines] = useState<string[]>([]);
  const [updateError, setUpdateError] = useState<string | null>(null);
  /** Which command produced the log above (install vs update), for its title. */
  const [consoleKind, setConsoleKind] = useState<ConsoleKind | null>(null);
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
    // Console output of the DSH install/update command, one line per event.
    const un3 = listen<string>("dsh://update", (e) => {
      setUpdateLines((prev) => {
        const next = [...prev, e.payload];
        // Keep the tail bounded: npm can be very chatty on a cold install.
        return next.length > 500 ? next.slice(next.length - 500) : next;
      });
    });
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
    };
  }, [refresh]);

  const checkUpdate = useCallback(() => {
    api
      .checkDshUpdate()
      .then(setUpdateInfo)
      .catch(() => {});
  }, []);

  const checkLauncherUpdate = useCallback(() => {
    api
      .checkLauncherUpdate()
      .then(setLauncherUpdateInfo)
      .catch(() => {});
  }, []);

  useEffect(() => {
    checkUpdate();
    checkLauncherUpdate();
  }, [checkUpdate, checkLauncherUpdate]);

  // "Refresh home" (top-right icon): behaves like reopening the app —
  // forces a full environment re-detection (Node.js / DSH / browsers) and
  // re-fetches the latest DSH version. The status and update checks are
  // independent: a failed network lookup still updates the local status.
  const handleRefresh = useCallback(async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      try {
        setStatus(await api.refreshStatus());
      } catch {
        /* backend not ready yet */
      }
      try {
        setUpdateInfo(await api.checkDshUpdate());
      } catch {
        /* network unreachable — keep the previous value */
      }
      try {
        setLauncherUpdateInfo(await api.checkLauncherUpdate());
      } catch {
        /* network unreachable — keep the previous value */
      }
    } finally {
      setRefreshing(false);
    }
  }, [refreshing]);

  const lang = initialLanguage(status?.config.language ?? null);

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
      /* best effort */
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
        launcherUpdateInfo={launcherUpdateInfo}
        updateLines={updateLines}
        updateError={updateError}
        consoleKind={consoleKind}
        setConsoleKind={setConsoleKind}
        setUpdateLines={setUpdateLines}
        setUpdateError={setUpdateError}
        checkUpdate={checkUpdate}
        refresh={refresh}
        onRefresh={handleRefresh}
        refreshing={refreshing}
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
  launcherUpdateInfo,
  updateLines,
  updateError,
  consoleKind,
  setConsoleKind,
  setUpdateLines,
  setUpdateError,
  checkUpdate,
  refresh,
  onRefresh,
  refreshing,
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
  launcherUpdateInfo: UpdateInfo | null;
  updateLines: string[];
  updateError: string | null;
  consoleKind: ConsoleKind | null;
  setConsoleKind: (kind: ConsoleKind | null) => void;
  setUpdateLines: (lines: string[]) => void;
  setUpdateError: (err: string | null) => void;
  checkUpdate: () => void;
  refresh: () => Promise<void>;
  onRefresh: () => void;
  refreshing: boolean;
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

  const currentStatus = status;
  const canOpen =
    currentStatus.process.state === "running" || currentStatus.env.ready;

  // ---- Main action: install or open (existing logic) ----
  async function mainAction() {
    try {
      if (canOpen) {
        setBusyPhase("opening");
        await api.openHarness();
      } else {
        // First install: show the live console log too (the backend streams
        // every npm line into `dsh://update`, same as an update), so a cold
        // install is never a silent spinner.
        setBusyPhase("installing");
        setConsoleKind("install");
        setUpdateLines([]);
        setUpdateError(null);
        try {
          const report = await api.ensureEnvironment();
          if (report.error) throw new Error(report.error);
        } catch (e) {
          // Keep the streamed log on screen: it is what explains the failure.
          setUpdateError(String(e));
          throw e;
        }
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

  // ---- Start service only (no browser) ----
  async function startService() {
    setBusyPhase("starting");
    try {
      await api.startDsh(false);
      await refresh();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusyPhase(null);
    }
  }

  // ---- Stop service ----
  async function stopService() {
    setBusyPhase("stopping");
    try {
      await api.stopDsh();
      await refresh();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusyPhase(null);
    }
  }

  // ---- Restart service ----
  async function restartService() {
    setBusyPhase("restarting");
    try {
      await api.restartDsh(false);
      await refresh();
    } catch (e) {
      notify(String(e));
    } finally {
      setBusyPhase(null);
    }
  }

  // ---- Update DSH ----
  // `targetVersion` is the release channel the user picked on the home page:
  // the newest build or the maintainers' recommended one. Omitted for a first
  // install, where the backend resolves the newest build itself.
  async function updateDsh(targetVersion?: string) {
    const wasRunning = currentStatus.process.state === "running";
    setBusyPhase("updating");
    // Fresh log for this attempt; the backend streams every npm line into it.
    setConsoleKind("update");
    setUpdateLines([]);
    setUpdateError(null);
    try {
      if (wasRunning) await api.stopDsh();
      const installed = await api.installDsh(targetVersion);
      await refresh();
      if (wasRunning) await api.restartDsh(false);
      await refresh();
      checkUpdate();
      notify(t("home.updated", { version: installed }));
    } catch (e) {
      // Keep the streamed console log on screen: the toast is gone in a few
      // seconds, but the npm output is what explains the failure.
      setUpdateError(String(e));
      notify(String(e));
    } finally {
      setBusyPhase(null);
    }
  }

  const inSettings = view === "settings";
  const inPlugins = view === "plugins";

  return (
    <div className="app">
      <div className="topbar">
        <button
          className="icon-btn"
          aria-label={t("app.refresh")}
          title={t("app.refresh")}
          onClick={onRefresh}
          disabled={refreshing}
        >
          {refreshing ? <span className="spinner" /> : <RefreshIcon />}
        </button>
        <button
          className={`icon-btn plugin${inPlugins ? " active" : ""}`}
          aria-label={inPlugins ? t("app.close_plugins") : t("app.plugins")}
          title={inPlugins ? t("app.close_plugins") : t("app.plugins")}
          onClick={() => setView(inPlugins ? "home" : "plugins")}
        >
          {inPlugins ? "←" : <PuzzleIcon />}
        </button>
        <button
          className={`icon-btn${inSettings ? " active" : ""}`}
          aria-label={inSettings ? t("app.close_settings") : t("app.settings")}
          title={inSettings ? t("app.close_settings") : t("app.settings")}
          onClick={() => setView(inSettings ? "home" : "settings")}
        >
          {inSettings ? "←" : <SettingsIcon />}
        </button>
      </div>

      <div className="main">
        {inSettings ? (
          <Settings status={status} notify={notify} onChanged={refresh} />
        ) : inPlugins ? (
          <Plugins notify={notify} onChanged={refresh} />
        ) : (
          <Home
            status={status}
            canOpen={canOpen}
            busyPhase={busyPhase}
            envProgress={envProgress}
            updateInfo={updateInfo}
            launcherUpdateInfo={launcherUpdateInfo}
            updateLines={updateLines}
            updateError={updateError}
            consoleKind={consoleKind}
            onDismissUpdateLog={() => {
              setUpdateLines([]);
              setUpdateError(null);
            }}
            onMainAction={mainAction}
            onUpdate={updateDsh}
            onStart={startService}
            onStop={stopService}
            onRestart={restartService}
            notify={notify}
          />
        )}
      </div>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
