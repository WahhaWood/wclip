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
  type Session,
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
/** When each queue item started cutting (ms), to measure `app.speed`. */
const cutStarted = new Map<number, number>();

export const app = $state({
  screen: "home" as Screen,
  /** Whether the speech model is downloaded; `null` until asked. */
  speechModel: null as boolean | null,
  settings: null as Settings | null,
  vod: null as Vod | null,
  /** Chat log picked for a local video. */
  chatLog: null as { path: string; info: ChatLogInfo } | null,
  rows: [] as Row[],
  /** Work of the last run, offered on the home screen until the user picks or starts anew. */
  session: null as Session | null,
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
  /** Video seconds cut per real second by one clip, measured while the job runs; `null` before a clip has run a while. */
  speed: null as number | null,
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

/** The work to keep between runs; `null` while there is nothing worth keeping. */
export function currentSession(): Session | null {
  const vod = app.vod;
  const rows = app.rows.filter((r) => r.start.trim() || r.end.trim() || r.name.trim());
  if (!vod || rows.length === 0) return null;
  return {
    input: vod.local ? vod.playlist : vod.id,
    local: vod.local,
    chatLog: app.chatLog?.path ?? null,
    rows: rows.map(({ start, end, name, captions, captionsFor }) => $state.snapshot({ start, end, name, captions, captionsFor })),
  };
}

let sessionTimer: ReturnType<typeof setTimeout> | undefined;

/** Keeps the current work a moment after the last change; an old session waits for the user's choice. */
export function saveSession() {
  clearTimeout(sessionTimer);
  sessionTimer = setTimeout(() => {
    const session = currentSession();
    if (session) api.sessionSave(session).catch(console.error);
  }, 800);
}

/** Opens the video of the saved session and puts its rows back; rejects like `vodInfo`. */
export async function resumeSession(session: Session) {
  app.session = null;
  app.vod = session.local ? await api.localVideo(session.input) : await api.vodInfo(session.input);
  rememberVod(app.vod);
  app.chatLog = null;
  app.rows = session.rows.map((r) => ({ ...newRow(r.start, r.end, r.name), captions: r.captions, captionsFor: r.captionsFor }));
  if (session.chatLog) {
    const path = session.chatLog;
    app.chatLog = { path, info: await api.chatLogOpen(path) };
  }
}

export function discardSession() {
  app.session = null;
  api.sessionClear().catch(console.error);
}

const RECENT_KEPT = 10;

/** Puts the opened video first in the list of recent ones. */
export function rememberVod(vod: Vod) {
  const s = app.settings;
  if (!s) return;
  const input = vod.local ? vod.playlist : vod.id;
  const entry = { input, local: vod.local, title: vod.title, channel: vod.channel, opened: Date.now() };
  s.recent = [entry, ...s.recent.filter((r) => r.input !== input)].slice(0, RECENT_KEPT);
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
    notify: s.notify,
  };
  for (const i of items) app.queue[i].status = { state: "waiting" };
  cutStarted.clear();
  app.speed = null;
  app.jobItems = items;
  await api.jobStart(job, overlay, s.outDir, s.nameTemplate, options);
  app.running = true;
}

/** Loads settings and starts listening to job events; call once on start. */
export async function initApp() {
  app.settings = await api.settingsLoad();
  app.session = await api.sessionLoad().catch(() => null);
  await listen<ClipEvent>("clip", ({ payload: e }) => {
    const item = app.queue[app.jobItems[e.index]];
    if (!item) return;
    const { index: _, ...status } = e;
    item.status = status;
    if (status.state === "running") {
      const at = app.jobItems[e.index];
      const started = cutStarted.get(at) ?? (cutStarted.set(at, Date.now()), Date.now());
      const elapsed = (Date.now() - started) / 1000;
      // Too early the number jumps about.
      if (elapsed >= 3 && status.fraction > 0.02) {
        app.speed = ((item.interval.end - item.interval.start) * status.fraction) / elapsed;
      }
    }
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
