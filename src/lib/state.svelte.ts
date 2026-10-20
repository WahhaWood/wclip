// App-wide state shared by the screens, so switching screens keeps everything.
import { listen } from "@tauri-apps/api/event";
import { api, type ClipEvent, type Settings, type Vod } from "./api";

export type Screen = "home" | "queue" | "settings";

/** An interval row as typed; times stay text until the job starts. */
export type Row = { id: number; start: string; end: string; name: string };

export type ClipState =
  | { state: "waiting" }
  | { state: "preparing" }
  | { state: "running"; fraction: number }
  | { state: "done"; path: string }
  | { state: "failed"; error: string };

export type QueueItem = { name: string; start: number; end: number; status: ClipState };

let nextRow = 1;

export const app = $state({
  screen: "home" as Screen,
  settings: null as Settings | null,
  vod: null as Vod | null,
  rows: [] as Row[],
  queue: [] as QueueItem[],
  running: false,
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

/** Loads settings and starts listening to job events; call once on start. */
export async function initApp() {
  app.settings = await api.settingsLoad();
  await listen<ClipEvent>("clip", ({ payload: e }) => {
    const item = app.queue[e.index];
    if (!item) return;
    const { index: _, ...status } = e;
    item.status = status;
  });
  await listen("job-done", () => {
    app.running = false;
  });
}
