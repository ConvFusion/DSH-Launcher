// Shared types mirroring the Rust backend (src-tauri).

export type ProcessState =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "error";

export interface ProcSnapshot {
  state: ProcessState;
  pid: number | null;
  host: string;
  port: number;
  url: string;
  error: string | null;
  error_details: string | null;
  started_at: string | null;
  output_tail: string[];
  /** True when running outside the launcher (e.g. started manually). */
  external: boolean;
}

export type NodeSource = "system" | "bundled";

export interface NodePub {
  version: string;
  source: NodeSource;
  path: string;
}

export interface DshPub {
  version: string;
  path: string;
}

export interface EnvStatus {
  node: NodePub | null;
  dsh: DshPub | null;
  ready: boolean;
}

export interface BrowserInfo {
  id: string;
  name: string;
  installed: boolean;
}

export interface BrowserStatus {
  selected: string | null;
  remember: boolean;
  detected: BrowserInfo[];
  system_default: string | null;
}

export interface Config {
  browser: {
    type: string | null;
    remember: boolean;
  };
  server: {
    host: string;
    port: number;
  };
  autostart: boolean;
  open_browser_on_start: boolean;
  language: string;
  theme: string;
  dsh_dir: string | null;
  node_path: string | null;
}

export interface LauncherStatus {
  process: ProcSnapshot;
  env: EnvStatus;
  browser: BrowserStatus;
  config: Config;
  launcher_version: string;
  data_dir: string;
}

export interface StartOutcome {
  ok: boolean;
  /** "ready" | "already_running" | "port_in_use" | "error" */
  kind: string;
  port: number | null;
  suggestions: number[];
  message: string | null;
  details: string | null;
}

export interface EnvProgress {
  stage: string;
  message: string;
  error: string | null;
  error_details: string | null;
}

export interface UpdateInfo {
  installed: string | null;
  /** Newest build published on any release channel. */
  latest: string | null;
  /** True when `latest` is worth installing over what is already there. */
  update_available: boolean;
  /**
   * The maintainers' recommended build (the npm `latest` dist-tag for DSH).
   * `null` for sources with a single channel — the launcher's own releases.
   */
  recommended: string | null;
  /** True when `recommended` is worth installing over what is already there. */
  recommended_available: boolean;
}

export interface PluginStatus {
  name: string;
  installed: boolean;
}

export interface EnvReport {
  ready: boolean;
  message: string | null;
  error: string | null;
  error_details: string | null;
}

/** What the home-page buttons are doing right now. */
export type MainPhase = "installing" | "opening" | "updating" | "starting" | "stopping" | "restarting";

/**
 * Which command the home page's live console log belongs to — a first install
 * or an update — so the panel stays titled correctly even after a failure,
 * when the busy phase is already over.
 */
export type ConsoleKind = "install" | "update";
