// Thin typed wrapper over Tauri IPC commands.
import { invoke } from "@tauri-apps/api/core";
import type {
  Config,
  EnvReport,
  LauncherStatus,
  PluginStatus,
  UpdateInfo,
} from "./types";

export const api = {
  getStatus: () => invoke<LauncherStatus>("get_status"),
  /** Force a full env re-detection (Node/DSH/browsers) + return fresh status. */
  refreshStatus: () => invoke<LauncherStatus>("refresh_status"),
  ensureEnvironment: () => invoke<EnvReport>("ensure_environment"),
  checkDshUpdate: () => invoke<UpdateInfo>("check_dsh_update"),
  installDsh: () => invoke<string>("install_dsh_package"),
  installPlugin: (name: string) => invoke<string>("install_dsh_plugin", { name }),
  removePlugin: (name: string) => invoke<string>("remove_dsh_plugin", { name }),
  pluginStatus: () => invoke<PluginStatus[]>("plugin_status"),

  startDsh: (openBrowser?: boolean) => invoke("start_dsh", { openBrowser }),
  stopDsh: () => invoke<void>("stop_dsh"),
  restartDsh: (openBrowser?: boolean) =>
    invoke("restart_dsh", { openBrowser }),
  openHarness: () => invoke<void>("open_harness"),

  updateConfig: (patch: {
    language?: string;
    theme?: string;
    node_path?: string | null;
  }) => invoke<Config>("update_config", { patch }),
  diagnoseEnvironment: () => invoke<string[]>("diagnose_environment"),
  /** Bundle all logs + config + diagnostics into a zip; returns its path. */
  collectLogs: () => invoke<string>("collect_logs"),
};
