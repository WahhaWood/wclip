// App-wide state shared by the screens, so switching screens keeps everything.
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type Caption,
  type ChatLogInfo,
  type ChatLogRef,
  type ClipEvent,
  type Failure,
  type Interval,
  type MergeEvent,
  type OverlayStyle,
  type Settings,
  type UpdateInfo,
  type UpdateProgress,
  type Vod,
} from "./api";
import { formatTime } from "./time";

export type Screen = "home" | "queue" | "layout" | "settings";

/**
 * An interval row as typed; times stay text until the job starts. `captions` are
 * the edited subtitles of the interval `captionsFor` (`start|end`), stale once it changes.
 */
export type Row = { id: number; start: string; end: string; name: string; captions: Caption[] | null; captionsFor: string | null };

export type ClipState =
  | { state: "waiting" }
  | { state: "preparing" }
  | { state: "transcribing"; fraction: number }
  | { state: "running"; fraction: number }
  | { state: "retrying"; attempt: number }
  | { state: "done"; path: string; noChat: boolean }
  | { state: "failed"; error: Failure };

/** `captions`: edited subtitles, else transcribed while cutting. */
export type QueueItem = { interval: Interval; status: ClipState; captions: Caption[] | null };

/** Chat of the whole video, for finding moments. */
export type ChatState =
  | { state: "idle" }
  | { state: "loading"; fraction: number }
  | { state: "ready"; count: number }
  /** `error` as thrown by the command, for `explain`. */
  | { state: "failed"; error: unknown };

export type UpdateState =
  | { state: "found" }
  | { state: "downloading"; fraction: number | null }
  | { state: "ready" }
  /** `error` as thrown by the command, for `explain`. */
  | { state: "failed"; error: unknown };

let nextRow = 1;

export const app = $state({
  screen: "home" as Screen,
  /** Whether the speech model is downloaded; `null` until asked. */
  speechModel: null as boolean | null,
  settings: null as Settings | null,
  vod: null as Vod | null,
  /** Chat log picked for a local video. */
  chatLog: null as { path: string; info: ChatLogInfo } | null,
  rows: [] as Row[],
  /** `key` (see `chatKey`) of the video the chat is for. */
  chat: { key: null as string | null, status: { state: "idle" } as ChatState },
  /** VOD of the queue: the home screen may already show another one. */
  queueVod: null as Vod | null,
  queueChatLog: null as ChatLogRef | null,
  queue: [] as QueueItem[],
  /** Joining the clips of the queue into one video, once started. */
  merge: null as MergeEvent | null,
  /** Queue index of each interval of the running job, by its index in the job. */
  jobItems: [] as number[],
  running: false,
  /** Release shown in the update banner; `null` hides it. */
  update: null as UpdateInfo | null,
  updateState: { state: "found" } as UpdateState,
});

export function newRow(start = "", end = "", name = ""): Row {
  return { id: nextRow++, start, end, name, captions: null, captionsFor: null };
}

/** Adds an interval row after the filled ones, dropping empty rows. */
export function addInterval(start: number, end: number, name = "") {
  const kept = app.rows.filter((r) => r.start.trim() || r.end.trim() || r.name.trim());
  app.rows = [...kept, newRow(formatTime(Math.round(start)), formatTime(Math.round(end)), name)];
}

/** Which chat a video has, as `chat_key` on the Rust side; `null` for a local video without a log. */
export function chatKey(vod: Vod, log: ChatLogRef | null): string | null {
  if (!vod.local) return `vod:${vod.id}`;
  return log ? `log:${log.path}@${log.start}` : null;
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;

/** Saves the settings a moment after the last change. */
export function saveSettings() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    if (app.settings) api.settingsSave($state.snapshot(app.settings)).catch(console.error);
  }, 400);
}

/**
 * Cuts queue items `items` (indices into `app.queue`) of `app.queueVod`, joining
 * them afterwards if `merge`; rejects with a `Failure`.
 */
export async function runJob(items: number[], overlay: OverlayStyle | null, merge = false) {
  const s = app.settings;
  if (!app.queueVod || !s) return;
  const job = {
    vod: $state.snapshot(app.queueVod),
    intervals: items.map((i) => $state.snapshot(app.queue[i].interval)),
    chat_log: $state.snapshot(app.queueChatLog),
    captions: items.map((i) => $state.snapshot(app.queue[i].captions)),
  };
  const options = {
    output: $state.snapshot(s.output),
    chatOutput: s.chatOutput,
    merge,
    filter: $state.snapshot(s.chatFilter),
    layout: s.vertical ? $state.snapshot(s.layout) : null,
    subtitles: s.subtitles ? $state.snapshot(s.subtitleStyle) : null,
    language: s.speechLanguage,
    speechGpu: s.speechGpu,
  };
  for (const i of items) app.queue[i].status = { state: "waiting" };
  app.jobItems = items;
  await api.jobStart(job, overlay, s.outDir, s.nameTemplate, options);
  app.running = true;
}

/** Loads settings and starts listening to job events; call once on start. */
export async function initApp() {
  app.settings = await api.settingsLoad();
  await listen<ClipEvent>("clip", ({ payload: e }) => {
    const item = app.queue[app.jobItems[e.index]];
    if (!item) return;
    const { index: _, ...status } = e;
    item.status = status;
  });
  await listen<MergeEvent>("merge", ({ payload }) => {
    app.merge = payload;
  });
  await listen<number>("chat-progress", ({ payload: fraction }) => {
    if (app.chat.status.state === "loading") app.chat.status = { state: "loading", fraction };
  });
  await listen("job-done", () => {
    app.running = false;
  });
  await listen<UpdateProgress>("update-progress", ({ payload: p }) => {
    app.updateState = { state: "downloading", fraction: p.total ? p.done / p.total : null };
  });
  // At most once a day (the Rust side keeps the stamp); offline stays silent.
  api
    .updateCheck(false)
    .then((u) => {
      if (u && u.version !== app.settings?.skippedVersion) showUpdate(u);
    })
    .catch(console.error);
}

export function showUpdate(u: UpdateInfo) {
  app.update = u;
  app.updateState = { state: "found" };
}
