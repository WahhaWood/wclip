// Types and commands of the Rust side (src-tauri/src/lib.rs). Field names follow serde.
import { invoke } from "@tauri-apps/api/core";

export type Tool = "ffmpeg" | "ffprobe" | "yt-dlp";
export type ToolInfo = { tool: Tool; path: string | null; version: string | null };
export type ToolProgress = { tool: Tool; done: number; total: number | null };

export type Vod = {
  id: string;
  title: string;
  channel: string;
  duration: number;
  thumbnail: string | null;
  playlist: string;
  height: number | null;
  /** A file on disk: `playlist` is its path, the chat comes from a chat log. */
  local: boolean;
  /** Naive local time (seconds) the file starts at, from its name. */
  started: number | null;
};

/** `clock`: times are naive local wall-clock seconds, else seconds of the VOD. */
export type ChatLogInfo = { channel: string; clock: boolean; start: number; count: number };
/** `start`: chat time at 0:00 of the video. */
export type ChatLogRef = { path: string; start: number };

export type Interval = { start: number; end: number; name: string };
export type Job = { vod: Vod; intervals: Interval[]; chat_log: ChatLogRef | null };

/** Positions and sizes are pixels at 1080p; (x, y) is the panel's top-left corner. */
export type OverlayStyle = {
  x: number;
  y: number;
  width: number;
  height: number;
  font: string;
  fontSize: number;
  background: number;
  lifetime: number;
};

export type Preset = { name: string; style: OverlayStyle };
export type Settings = {
  outDir: string | null;
  nameTemplate: string;
  chat: boolean;
  overlay: OverlayStyle;
  presets: Preset[];
  /** Preset picked last; edits to `overlay` go into it too. */
  activePreset: string | null;
  skippedVersion: string | null;
};

/** A release newer than the running app; `installable` false means linking to `page`. */
export type UpdateInfo = { version: string; notes: string; page: string; installable: boolean };
export type UpdateProgress = { done: number; total: number | null };

export type FailureKind = "vod-missing" | "sub-only" | "network" | "disk-full" | "chat" | "other";
/** Error of commands and failed clips; turn it into words with `explain` (errors.ts). */
export type Failure = { kind: FailureKind; detail: string };

export type ClipEvent =
  | { state: "preparing"; index: number }
  | { state: "running"; index: number; fraction: number }
  | { state: "retrying"; index: number; attempt: number }
  | { state: "done"; index: number; path: string; noChat: boolean }
  | { state: "failed"; index: number; error: Failure };

export const api = {
  toolsStatus: () => invoke<ToolInfo[]>("tools_status"),
  toolsInstall: () => invoke<ToolInfo[]>("tools_install"),
  vodInfo: (input: string) => invoke<Vod>("vod_info", { input }),
  localVideo: (path: string) => invoke<Vod>("local_video", { path }),
  chatLogOpen: (path: string) => invoke<ChatLogInfo>("chat_log_open", { path }),
  settingsLoad: () => invoke<Settings>("settings_load"),
  settingsSave: (settings: Settings) => invoke<void>("settings_save", { settings }),
  fontFamilies: () => invoke<string[]>("font_families"),
  previewFrame: (playlist: string, time: number) => invoke<ArrayBuffer>("preview_frame", { playlist, time }),
  overlayPanel: (style: OverlayStyle) => invoke<ArrayBuffer>("overlay_panel", { style }),
  jobStart: (job: Job, overlay: OverlayStyle | null, outDir: string | null, nameTemplate: string) =>
    invoke<void>("job_start", { job, overlay, outDir, nameTemplate }),
  jobCancel: () => invoke<void>("job_cancel"),
  errorLog: () => invoke<string | null>("error_log"),
  updateCheck: (force: boolean) => invoke<UpdateInfo | null>("update_check", { force }),
  updateInstall: () => invoke<void>("update_install"),
  updateRestart: () => invoke<void>("update_restart"),
};
