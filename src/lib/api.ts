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
  /** Audio-only HLS playlist, for speech recognition. */
  audio: string | null;
  /** A file on disk: `playlist` is its path, the chat comes from a chat log. */
  local: boolean;
  /** Naive local time (seconds) the file starts at, from its name. */
  started: number | null;
};

/** `clock`: times are naive local wall-clock seconds, else seconds of the VOD. */
export type ChatLogInfo = { channel: string; clock: boolean; start: number; count: number };
/** `start`: chat time at 0:00 of the video. */
export type ChatLogRef = { path: string; start: number };

/** A burst of chat, seconds of the video; `words` are [word, messages using it]. */
export type Peak = { start: number; end: number; count: number; words: [string, number][] };
/** Messages per `bar` seconds from 0:00; peaks strongest first. */
export type Activity = { bar: number; counts: number[]; total: number; peaks: Peak[] };

export type Interval = { start: number; end: number; name: string };
/** `captions`: edited captions by interval; `null` ones are transcribed while cutting. */
export type Job = { vod: Vod; intervals: Interval[]; chat_log: ChatLogRef | null; captions: (Caption[] | null)[] };

/** A caption on screen, seconds of the clip; `words` are when each word of `text` starts. */
export type Caption = { start: number; end: number; text: string; words: number[] };
/** Sizes are pixels on a frame whose short side is 1080; `x`, `y` are for 16:9 clips. */
export type SubtitleStyle = {
  font: string;
  size: number;
  color: [number, number, number];
  /** The word being said. */
  highlight: [number, number, number];
  outline: number;
  uppercase: boolean;
  /** Most words in a caption. */
  words: number;
  /** Middle of the caption, fractions of the frame's width and height. */
  x: number;
  y: number;
};

/** Positions and sizes are pixels at 1080p; (x, y) is the panel's top-left corner. */
export type OverlayStyle = {
  x: number;
  y: number;
  width: number;
  height: number;
  font: string;
  fontSize: number;
  background: number;
  /** Opacity of a plate under each message, 0..1; 0 draws none. */
  messageBackground: number;
  lifetime: number;
};

/** Messages the overlay leaves out; `bots` are names, any case. */
export type ChatFilter = { hideBots: boolean; bots: string[]; hideCommands: boolean };

export type Preset = { name: string; style: OverlayStyle };

/** Part of the source frame, as fractions of its width and height. */
export type Area = { x: number; y: number; w: number; h: number };
/** A part of the source on the 1080x1920 canvas; its height follows the proportions of `src`. */
export type Piece = { name: string; src: Area; x: number; y: number; width: number };
/** The chat panel on the canvas, in canvas pixels. */
export type ChatBox = { x: number; y: number; width: number; height: number; fontSize: number };
/** A vertical clip (see src-tauri/src/layout.rs); `pieces` bottom first. */
export type Layout = {
  name: string;
  pieces: Piece[];
  blur: boolean;
  color: [number, number, number];
  chat: ChatBox;
  /** Middle of the subtitles, fractions of the canvas width and height. */
  subtitlesX: number;
  subtitlesY: number;
};

/** `auto`: the first GPU encoder that works, else `cpu` (libx264 / libx265). */
export type Encoder = "auto" | "cpu" | "nvenc" | "qsv" | "amf" | "vaapi";
export type Codec = "h264" | "hevc";
export type Quality = "high" | "medium" | "small";
export type Output = { encoder: Encoder; codec: Codec; quality: Quality };
/** `file`: the chat as its own video with transparency (ProRes 4444 `.chat.mov`). */
export type ChatOutput = "burn" | "file" | "both";
export type JobOptions = {
  output: Output;
  chatOutput: ChatOutput;
  merge: boolean;
  filter: ChatFilter;
  /** Lay the clips out vertically. */
  layout: Layout | null;
  /** Burn in subtitles of the speech so. */
  subtitles: SubtitleStyle | null;
  /** Language of the speech, like "ru"; `null` detects it. */
  language: string | null;
};
export type Settings = {
  outDir: string | null;
  nameTemplate: string;
  chat: boolean;
  overlay: OverlayStyle;
  presets: Preset[];
  /** Preset picked last; edits to `overlay` go into it too. */
  activePreset: string | null;
  skippedVersion: string | null;
  output: Output;
  chatOutput: ChatOutput;
  /** Also join the clips of a job into one video. */
  merge: boolean;
  chatFilter: ChatFilter;
  /** Value of the `auth-token` cookie of twitch.tv, for subscriber-only VODs. */
  twitchToken: string | null;
  /** Cut vertical clips laid out by `layout`. */
  vertical: boolean;
  layout: Layout;
  /** Saved layouts. */
  layouts: Layout[];
  /** Name of the saved layout picked last for a channel. */
  channelLayouts: Record<string, string>;
  /** Burn in subtitles of the speech. */
  subtitles: boolean;
  subtitleStyle: SubtitleStyle;
  /** Like "ru"; `null` detects the language. */
  speechLanguage: string | null;
  /** Recognise the speech on the GPU when there is one. */
  speechGpu: boolean;
  /** Notify by the system when a job ends. */
  notify: boolean;
  /** Videos opened last, newest first. */
  recent: Recent[];
};

/** A video opened before: `input` is what `vodInfo` or `localVideo` takes. */
export type Recent = { input: string; local: boolean; title: string; channel: string; opened: number };

/** A release newer than the running app; `installable` false means linking to `page`. */
export type UpdateInfo = { version: string; notes: string; page: string; installable: boolean };
export type UpdateProgress = { done: number; total: number | null };

export type FailureKind = "vod-missing" | "sub-only" | "bad-token" | "no-model" | "network" | "disk-full" | "chat" | "other";
/** Error of commands and failed clips; turn it into words with `explain` (errors.ts). */
export type Failure = { kind: FailureKind; detail: string };

export type ClipEvent =
  | { state: "preparing"; index: number }
  | { state: "transcribing"; index: number; fraction: number }
  | { state: "running"; index: number; fraction: number }
  | { state: "retrying"; index: number; attempt: number }
  | { state: "done"; index: number; path: string; noChat: boolean }
  | { state: "failed"; index: number; error: Failure };

export type MergeEvent = { state: "running" } | { state: "done"; path: string } | { state: "failed"; error: Failure };

/** Unfinished work kept between runs: the video as `vodInfo`/`localVideo` take it, and the typed rows. */
export type Session = {
  input: string;
  local: boolean;
  chatLog: string | null;
  rows: { start: string; end: string; name: string; captions: Caption[] | null; captionsFor: string | null }[];
};

export const api = {
  toolsStatus: () => invoke<ToolInfo[]>("tools_status"),
  toolsInstall: () => invoke<ToolInfo[]>("tools_install"),
  vodInfo: (input: string) => invoke<Vod>("vod_info", { input }),
  localVideo: (path: string) => invoke<Vod>("local_video", { path }),
  chatLogOpen: (path: string) => invoke<ChatLogInfo>("chat_log_open", { path }),
  /** Chat of the whole video, kept on the Rust side; reports `chat-progress` (0..1). */
  chatLoad: (vod: Vod, chatLog: ChatLogRef | null) => invoke<number>("chat_load", { vod, chatLog }),
  chatActivity: (query: string) => invoke<Activity>("chat_activity", { query }),
  settingsLoad: () => invoke<Settings>("settings_load"),
  settingsSave: (settings: Settings) => invoke<void>("settings_save", { settings }),
  sessionLoad: () => invoke<Session | null>("session_load"),
  sessionSave: (session: Session) => invoke<void>("session_save", { session }),
  sessionClear: () => invoke<void>("session_clear"),
  fontFamilies: () => invoke<string[]>("font_families"),
  previewFrame: (playlist: string, time: number) => invoke<ArrayBuffer>("preview_frame", { playlist, time }),
  overlayPanel: (style: OverlayStyle) => invoke<ArrayBuffer>("overlay_panel", { style }),
  /** Working GPU encoders for `codec`; tried on first call (about a second). */
  encoders: (codec: Codec) => invoke<Encoder[]>("encoders", { codec }),
  jobStart: (job: Job, overlay: OverlayStyle | null, outDir: string | null, nameTemplate: string, options: JobOptions) =>
    invoke<void>("job_start", { job, overlay, outDir, nameTemplate, options }),
  jobCancel: () => invoke<void>("job_cancel"),
  report: () => invoke<string>("report"),
  errorLog: () => invoke<string | null>("error_log"),
  updateCheck: (force: boolean) => invoke<UpdateInfo | null>("update_check", { force }),
  updateInstall: () => invoke<void>("update_install"),
  updateRestart: () => invoke<void>("update_restart"),
  speechModelReady: () => invoke<boolean>("speech_model_ready"),
  /** Downloads the speech model (~900 MB); reports `model-progress` ({ done, total }). */
  speechModelInstall: () => invoke<void>("speech_model_install"),
  /** GPU the speech is recognised on; `null`: the CPU. */
  speechDevice: () => invoke<string | null>("speech_device"),
  /** Captions of `start..end` for editing; reports `transcribe-progress` (percent). A newer call cancels this one. */
  transcribe: (vod: Vod, start: number, end: number, language: string | null, gpu: boolean, words: number) =>
    invoke<Caption[]>("transcribe", { vod, start, end, language, gpu, words }),
};
