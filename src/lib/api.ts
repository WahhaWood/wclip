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
};

export type Interval = { start: number; end: number; name: string };
export type Job = { vod: Vod; intervals: Interval[] };

export type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";
export type OverlayStyle = {
  corner: Corner;
  width: number;
  height: number;
  margin: number;
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
};

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
  settingsLoad: () => invoke<Settings>("settings_load"),
  settingsSave: (settings: Settings) => invoke<void>("settings_save", { settings }),
  fontFamilies: () => invoke<string[]>("font_families"),
  overlayPreview: (playlist: string | null, time: number, style: OverlayStyle) =>
    invoke<ArrayBuffer>("overlay_preview", { playlist, time, style }),
  jobStart: (job: Job, overlay: OverlayStyle | null, outDir: string | null, nameTemplate: string) =>
    invoke<void>("job_start", { job, overlay, outDir, nameTemplate }),
  jobCancel: () => invoke<void>("job_cancel"),
};
