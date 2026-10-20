// App-wide state shared by the screens, so switching screens keeps everything.
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type ClipEvent,
  type Failure,
  type Interval,
  type OverlayStyle,
  type Settings,
  type UpdateInfo,
  type UpdateProgress,
  type Vod,
} from "./api";

export type Screen = "home" | "queue" | "settings";

/** An interval row as typed; times stay text until the job starts. */
export type Row = { id: number; start: string; end: string; name: string };

export type ClipState =
  | { state: "waiting" }
  | { state: "preparing" }
  | { state: "running"; fraction: number }
  | { state: "retrying"; attempt: number }
  | { state: "done"; path: string; noChat: boolean }
  | { state: "failed"; error: Failure };

export type QueueItem = { interval: Interval; status: ClipState };

export type UpdateState =
  | { state: "found" }
  | { state: "downloading"; fraction: number | null }
  | { state: "ready" }
  /** `error` as thrown by the command, for `explain`. */
  | { state: "failed"; error: unknown };

let nextRow = 1;

export const app = $state({
  screen: "home" as Screen,
  settings: null as Settings | null,
  vod: null as Vod | null,
  rows: [] as Row[],
  /** VOD of the queue: the home screen may already show another one. */
  queueVod: null as Vod | null,
  queue: [] as QueueItem[],
  /** Queue index of each interval of the running job, by its index in the job. */
  jobItems: [] as number[],
  running: false,
  /** Release shown in the update banner; `null` hides it. */
  update: null as UpdateInfo | null,
  updateState: { state: "found" } as UpdateState,
});

export function newRow(start = "", end = "", name = ""): Row {
  return { id: nextRow++, start, end, name };
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;

/** Saves the settings a moment after the last change. */
export function saveSettings() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    if (app.settings) api.settingsSave($state.snapshot(app.settings)).catch(console.error);
  }, 400);
}

/** Cuts queue items `items` (indices into `app.queue`) of `app.queueVod`; rejects with a `Failure`. */
export async function runJob(items: number[], overlay: OverlayStyle | null) {
  const s = app.settings;
  if (!app.queueVod || !s) return;
  const job = { vod: $state.snapshot(app.queueVod), intervals: items.map((i) => $state.snapshot(app.queue[i].interval)) };
  for (const i of items) app.queue[i].status = { state: "waiting" };
  app.jobItems = items;
  await api.jobStart(job, overlay, s.outDir, s.nameTemplate);
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
