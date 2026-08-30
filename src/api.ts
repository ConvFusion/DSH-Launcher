// Thin typed wrapper over Tauri IPC commands.
import { invoke } from "@tauri-apps/api/core";
import type {
  Config,
  EnvReport,
  LauncherStatus,
  UpdateInfo,
} from "./types";

export const api = {
  getStatus: () => invoke<LauncherStatus>("get_status"),
  ensureEnvironment: () => invoke<EnvReport>("ensure_environment"),
  checkDshUpdate: () => invoke<UpdateInfo>("check_dsh_update"),
  installDsh: () => invoke<string>("install_dsh_package"),

  startDsh: (openBrowser?: boolean) => invoke("start_dsh", { openBrowser }),
  stopDsh: () => invoke<void>("stop_dsh"),
  restartDsh: (openBrowser?: boolean) =>
    invoke("restart_dsh", { openBrowser }),
  openHarness: () => invoke<void>("open_harness"),

  updateConfig: (patch: { language?: string; theme?: string }) =>
    invoke<Config>("update_config", { patch }),
};
