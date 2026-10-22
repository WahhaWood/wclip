<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type Caption, type Interval } from "$lib/api";
  import { explain } from "$lib/errors";
  import { copyLayout } from "$lib/layouts";
  import { app, discardSession, newRow, rememberVod, resumeSession, runJob, type Row } from "$lib/state.svelte";
  import { formatClock, formatTime, intervalWarning, parseClock, parseIntervalList, parseTime } from "$lib/time";
  import Captions from "./Captions.svelte";
  import Frame from "./Frame.svelte";
  import Moments from "./Moments.svelte";

  const VIDEO = ["mp4", "mkv", "mov", "flv", "ts", "webm", "avi"];
  const CHAT_OUTPUT = { burn: "Чат в кадре", file: "Чат отдельным файлом", both: "Чат в кадре и файлом" };
  const CHAT_LOG = ["log", "json", "gz", "txt"];

  let link = $state("");
  let loading = $state(false);
  let error = $state<ReturnType<typeof explain> | null>(null);
  let starting = $state(false);
  let pasteOpen = $state(false);
  let pasteText = $state("");
  let pasteBad = $state<string[]>([]);
  let dragging = $state(false);
  /** Chat time at 0:00 of a local video, as typed. */
  let syncText = $state("");
  /** Row whose start and end frames are shown. */
  let editing = $state<number | null>(null);
  /** Row whose subtitles text is shown. */
  let texting = $state<number | null>(null);

  /** `field` is the input to highlight. An empty row is neither valid nor an error. */
  type Check = { interval: Interval | null; error: string | null; field: "start" | "end" | null };

  function check(row: Row, duration: number): Check {
    const fail = (field: "start" | "end", error: string): Check => ({ interval: null, error, field });
    const start = parseTime(row.start);
    const end = parseTime(row.end);
    if (!row.start.trim() && !row.end.trim()) return { interval: null, error: null, field: null };
    if (start === null) return fail("start", "Неверное время начала");
    if (end === null) return fail("end", "Неверное время конца");
    if (start >= end) return fail("end", "Конец должен быть позже начала");
    if (end > duration) return fail("end", `Запись длится ${formatTime(duration)}`);
    return { interval: { start, end, name: row.name.trim() }, error: null, field: null };
  }

  const checks = $derived(app.vod ? app.rows.map((r) => check(r, app.vod!.duration)) : []);
  const intervals = $derived(checks.flatMap((c) => (c.interval ? [c.interval] : [])));
  const syncStart = $derived.by(() => {
    const log = app.chatLog;
    if (!log) return null;
    return log.info.clock ? parseClock(syncText, log.info.start) : parseTime(syncText);
  });
  const chatLogRef = $derived(
    app.vod?.local && app.chatLog && syncStart !== null ? { path: app.chatLog.path, start: syncStart } : null,
  );
  /** A local video without a chat log is cut without chat. */
  const withChat = $derived(!!app.settings?.chat && (!app.vod?.local || !!app.chatLog));
  const ready = $derived(
    !!app.vod &&
      intervals.length > 0 &&
      checks.every((c) => !c.error) &&
      !(withChat && app.vod.local && syncStart === null) &&
      !app.running,
  );

  const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;
  const extension = (path: string) => baseName(path).split(".").pop()?.toLowerCase() ?? "";

  onMount(() => {
    const unlisten = getCurrentWebview().onDragDropEvent(({ payload: e }) => {
      dragging = e.type === "enter" || e.type === "over";
      if (e.type !== "drop") return;
      const video = e.paths.find((p) => VIDEO.includes(extension(p)));
      const log = e.paths.find((p) => CHAT_LOG.includes(extension(p)));
      // Video first: a chat log belongs to the video dropped with it.
      (video ? openLocal(video) : Promise.resolve()).then(() => {
        if (log && app.vod?.local) openChatLog(log);
      });
    });
    return () => unlisten.then((f) => f());
  });

  async function resume() {
    if (!app.session) return;
    loading = true;
    error = null;
    try {
      await resumeSession(app.session);
      link = "";
    } catch (e) {
      error = explain(e);
    } finally {
      loading = false;
    }
  }

  function openRecent(id: string) {
    link = id;
    return openVod();
  }

  async function openVod() {
    if (!link.trim()) return;
    loading = true;
    error = null;
    try {
      app.vod = await api.vodInfo(link);
      rememberVod(app.vod);
      app.chatLog = null;
      pickChannelLayout(app.vod.channel);
      if (app.rows.length === 0) app.rows = [newRow()];
    } catch (e) {
      error = explain(e);
    } finally {
      loading = false;
    }
  }

  /** The layout saved for `channel` last, if any. */
  function pickChannelLayout(channel: string) {
    const s = app.settings;
    const saved = s?.layouts.find((l) => l.name === s.channelLayouts[channel]);
    if (s && saved) s.layout = copyLayout(saved);
  }

  async function pickVideo() {
    const path = await open({ filters: [{ name: "Видео", extensions: VIDEO }] });
    if (typeof path === "string") await openLocal(path);
  }

  /** A video from disk; one interval over the whole file if none is typed yet. */
  async function openLocal(path: string) {
    loading = true;
    error = null;
    try {
      app.vod = await api.localVideo(path);
      rememberVod(app.vod);
      app.chatLog = null;
      link = "";
      if (app.rows.every((r) => !r.start.trim() && !r.end.trim())) {
        app.rows = [newRow("0:00", formatTime(Math.floor(app.vod.duration)))];
      }
    } catch (e) {
      error = explain(e);
    } finally {
      loading = false;
    }
  }

  async function pickChatLog() {
    const path = await open({ filters: [{ name: "Лог чата", extensions: CHAT_LOG }] });
    if (typeof path === "string") await openChatLog(path);
  }

  async function openChatLog(path: string) {
    error = null;
    try {
      const info = await api.chatLogOpen(path);
      app.chatLog = { path, info };
      // The file name of an OBS recording tells when it started.
      const start = info.clock ? (app.vod?.started ?? info.start) : info.start;
      syncText = info.clock ? formatClock(start) : formatTime(start);
    } catch (e) {
      error = explain(e);
    }
  }

  /** Moves a row's start or end by `d` seconds, within the video. */
  function nudge(row: Row, field: "start" | "end", d: number) {
    const t = parseTime(row[field]);
    if (t === null || !app.vod) return;
    row[field] = formatTime(Math.max(0, Math.min(t + d, app.vod.duration)));
  }

  function addPasted() {
    const { intervals, bad } = parseIntervalList(pasteText);
    const kept = app.rows.filter((r) => r.start.trim() || r.end.trim() || r.name.trim());
    const added = intervals.map((i) => newRow(formatTime(i.start), formatTime(i.end), i.name));
    app.rows = [...kept, ...added];
    pasteBad = bad;
    if (bad.length === 0) {
      pasteText = "";
      pasteOpen = false;
    }
  }

  async function chooseDir() {
    if (!app.settings) return;
    const dir = await open({ directory: true, defaultPath: app.settings.outDir ?? undefined });
    if (typeof dir === "string") app.settings.outDir = dir;
  }

  /** Key of `interval` for `Row.captionsFor`. */
  const captionsKey = (interval: Interval) => `${interval.start}|${interval.end}`;

  /** The row's edited captions, if they are for its current times. */
  function captionsOf(row: Row, interval: Interval): Caption[] | null {
    return row.captions && row.captionsFor === captionsKey(interval) ? $state.snapshot(row.captions) : null;
  }

  async function start() {
    const s = app.settings;
    if (!app.vod || !s) return;
    starting = true;
    error = null;
    try {
      app.queueVod = app.vod;
      app.queueChatLog = chatLogRef;
      app.queue = app.rows.flatMap((row, i) => {
        const interval = checks[i]?.interval;
        return interval ? [{ interval, status: { state: "waiting" as const }, captions: captionsOf(row, interval) }] : [];
      });
      app.merge = null;
      await runJob(
        intervals.map((_, i) => i),
        withChat ? $state.snapshot(s.overlay) : null,
        s.merge && intervals.length > 1,
      );
      app.screen = "queue";
    } catch (e) {
      error = explain(e);
    } finally {
      starting = false;
    }
  }
</script>

<div class="screen">
  <section class="glass card" class:drop={dragging}>
    <h2>Запись</h2>
    <form
      class="link"
      onsubmit={(e) => {
        e.preventDefault();
        openVod();
      }}
    >
      <input class="field" placeholder="https://www.twitch.tv/videos/…" bind:value={link} />
      <button class="btn primary" disabled={loading || !link.trim()}>
        {loading ? "Открываем…" : "Открыть"}
      </button>
      <button type="button" class="btn" disabled={loading} onclick={pickVideo}>Файл…</button>
    </form>
    <p class="dim hint">Ссылка на запись Twitch или видеофайл с диска, его можно перетащить в окно вместе с логом чата.</p>
    {#if error}
      <p class="error" title={error.detail}>{error.text}</p>
    {/if}
    {#if app.session && !app.vod}
      <div class="card session">
        <span>
          Прошлая работа не закончена: <bdi>{app.session.input}</bdi>, интервалов {app.session.rows.length}.
        </span>
        <button class="btn small primary" disabled={loading} onclick={resume}>Продолжить</button>
        <button class="btn small" disabled={loading} onclick={discardSession}>Начать заново</button>
      </div>
    {/if}

    {#if !app.vod && !app.session && app.settings?.recent.length}
      <div class="recent">
        <span class="dim">Недавние</span>
        {#each app.settings?.recent ?? [] as r (r.input)}
          <button class="btn recent-item" disabled={loading} onclick={() => (r.local ? openLocal(r.input) : openRecent(r.input))}>
            <span class="title">{r.title || baseName(r.input)}</span>
            <span class="dim">{r.local ? "Файл с диска" : r.channel} · {new Date(r.opened).toLocaleDateString("ru-RU")}</span>
          </button>
        {/each}
      </div>
    {/if}

    {#if app.vod}
      <div class="vod">
        {#if app.vod.thumbnail}
          <img src={app.vod.thumbnail} alt="" />
        {/if}
        <div>
          <div class="title">{app.vod.title}</div>
          <div class="dim">
            {app.vod.local ? "Файл с диска" : app.vod.channel} · {formatTime(Math.floor(app.vod.duration))}
          </div>
        </div>
      </div>

      {#if app.vod.local}
        <div class="chatlog">
          <span class="dim">Чат:</span>
          {#if app.chatLog}
            <span class="path" title={app.chatLog.path}><bdi>{baseName(app.chatLog.path)}</bdi></span>
            <span class="dim nowrap">
              · {app.chatLog.info.count} сообщ.{app.chatLog.info.channel ? ` · ${app.chatLog.info.channel}` : ""}
            </span>
          {:else}
            <span class="dim">лог Chatterino (.log) или TwitchDownloader (.json, .json.gz)</span>
          {/if}
          <button class="btn small" onclick={pickChatLog}>{app.chatLog ? "Другой…" : "Выбрать…"}</button>
        </div>
        {#if app.chatLog}
          <div class="sync">
            <span class="dim">Начало видео в чате</span>
            <input class="field" class:invalid={syncStart === null} bind:value={syncText} />
            <span class="dim hint">
              {app.chatLog.info.clock
                ? "Дата и время по часам лога. Чат отстаёт — уменьшите, спешит — увеличьте."
                : "Время записи Twitch, с которого начинается файл."}
            </span>
          </div>
        {/if}
      {/if}
    {/if}
  </section>

  {#if app.vod}
    <Moments vod={app.vod} chatLog={chatLogRef} />

    <section class="glass card">
      <div class="head">
        <h2>Интервалы</h2>
        <div class="actions">
          <button class="btn small" onclick={() => (pasteOpen = !pasteOpen)}>Вставить списком</button>
          <button class="btn small" onclick={() => app.rows.push(newRow())}>+ Интервал</button>
        </div>
      </div>

      {#if pasteOpen}
        <div class="paste">
          <textarea
            class="field"
            rows="5"
            placeholder={"По строке на интервал: начало, конец и имя клипа, например\n1:02:10 1:03:00 Лучший момент\n1:15:30 - 1:16:05"}
            bind:value={pasteText}
          ></textarea>
          {#if pasteBad.length}
            <p class="error">Не разобраны строки: {pasteBad.join("; ")}</p>
          {/if}
          <button class="btn small primary" onclick={addPasted} disabled={!pasteText.trim()}>Добавить</button>
        </div>
      {/if}

      <div class="rows">
        <div class="row header dim">
          <span>№</span><span>Начало</span><span>Конец</span><span>Длина</span><span>Имя клипа</span><span></span><span
          ></span><span></span>
        </div>
        {#each app.rows as row, i (row.id)}
          {@const c = checks[i]}
          <div class="row">
            <span class="dim">{i + 1}</span>
            <input class="field" class:invalid={c?.field === "start"} placeholder="0:00:00" bind:value={row.start} />
            <input class="field" class:invalid={c?.field === "end"} placeholder="0:00:30" bind:value={row.end} />
            <span class="dim">{c?.interval ? formatTime(c.interval.end - c.interval.start) : "—"}</span>
            <input class="field" placeholder="необязательно" bind:value={row.name} />
            <button
              class="btn small"
              class:selected={editing === row.id}
              title="Подогнать начало и конец по кадрам"
              disabled={!c?.interval}
              onclick={() => (editing = editing === row.id ? null : row.id)}>Кадры</button
            >
            {#if app.settings?.subtitles}
              <button
                class="btn small"
                class:selected={texting === row.id}
                title="Распознать речь и поправить текст субтитров"
                disabled={!c?.interval}
                onclick={() => (texting = texting === row.id ? null : row.id)}>Текст</button
              >
            {:else}
              <span></span>
            {/if}
            <button class="btn small" title="Удалить" onclick={() => app.rows.splice(i, 1)}>✕</button>
          </div>
          {#if c?.error}
            <p class="error row-error">{c.error}</p>
          {:else if c?.interval && intervalWarning(c.interval.start, c.interval.end)}
            <p class="dim row-error">{intervalWarning(c.interval.start, c.interval.end)}</p>
          {/if}
          {#if texting === row.id && c?.interval && app.settings?.subtitles}
            <Captions {row} interval={c.interval} />
          {/if}
          {#if editing === row.id && c?.interval}
            <div class="frames">
              {#each [["start", "Начало", c.interval.start], ["end", "Конец", c.interval.end]] as const as [field, label, t] (field)}
                <div class="frame-edit">
                  <!-- The end is exclusive: its last frame is just before it. -->
                  <Frame playlist={app.vod.playlist} time={field === "end" ? Math.max(0, t - 0.05) : t} />
                  <div class="nudge">
                    <span class="dim">{label} {formatTime(t)}</span>
                    {#each [-5, -1, 1, 5] as d (d)}
                      <button class="btn small" onclick={() => nudge(row, field, d)}>{d > 0 ? `+${d}` : d}</button>
                    {/each}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        {/each}
      </div>
    </section>

    {#if app.settings}
      <section class="glass card footer">
        <label class="toggle">
          <input type="checkbox" bind:checked={app.settings.chat} />
          {CHAT_OUTPUT[app.settings.chatOutput]}
          {#if app.settings.chat && app.vod.local && !app.chatLog}
            <span class="dim">· нет лога чата</span>
          {:else if app.settings.chat && app.settings.activePreset}
            <span class="dim">· {app.settings.activePreset}</span>
          {/if}
        </label>
        <label class="toggle" title="Речь стримера текстом в кадре">
          <input type="checkbox" bind:checked={app.settings.subtitles} />
          Субтитры
        </label>
        <div class="format">
          <button class="btn small" class:selected={!app.settings.vertical} onclick={() => app.settings && (app.settings.vertical = false)}>
            16:9
          </button>
          <button class="btn small" class:selected={app.settings.vertical} onclick={() => app.settings && (app.settings.vertical = true)}>
            9:16
          </button>
          {#if app.settings.vertical}
            <button class="btn small" title="Изменить раскладку" onclick={() => (app.screen = "layout")}>
              {app.settings.layout.name || "Раскладка"}
            </button>
          {/if}
        </div>
        {#if intervals.length > 1}
          <label class="toggle" title="Клипы останутся и по отдельности">
            <input type="checkbox" bind:checked={app.settings.merge} />
            Склеить
          </label>
        {/if}
        <div class="out">
          <span class="dim">Папка:</span>
          <!-- rtl clips the start of a long path; bdi keeps the path itself left-to-right. -->
          <span class="path" title={app.settings.outDir}><bdi>{app.settings.outDir}</bdi></span>
          <button class="btn small" onclick={chooseDir}>Изменить</button>
        </div>
        <button class="btn primary" disabled={!ready || starting} onclick={start}>
          {app.running ? "Идёт нарезка…" : `Нарезать${intervals.length ? ` (${intervals.length})` : ""}`}
        </button>
      </section>
    {/if}
  {/if}
</div>

<style>
  .screen {
    display: grid;
    gap: 16px;
  }

  .link {
    display: flex;
    gap: 10px;
  }

  .vod {
    display: flex;
    gap: 16px;
    align-items: center;
    margin-top: 16px;
  }

  .drop {
    outline: 2px dashed var(--primary);
    outline-offset: -2px;
  }

  .hint {
    font-size: 13px;
  }

  .session {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .recent {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .recent-item {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    text-align: left;
  }

  .recent-item .title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .recent-item .dim {
    flex: none;
  }

  .session span {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .chatlog,
  .sync {
    display: flex;
    gap: 10px;
    align-items: center;
    margin-top: 14px;
  }

  .chatlog .path {
    min-width: 0;
  }

  .sync .field {
    width: 190px;
  }

  .nowrap {
    white-space: nowrap;
  }

  .vod img {
    width: 176px;
    aspect-ratio: 16 / 9;
    object-fit: cover;
    border-radius: 12px;
  }

  .title {
    font-weight: 600;
    margin-bottom: 4px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }

  .actions {
    display: flex;
    gap: 8px;
  }

  .paste {
    display: grid;
    gap: 8px;
    justify-items: start;
    margin-bottom: 16px;
  }

  .paste textarea {
    resize: vertical;
  }

  .rows {
    display: grid;
    gap: 8px;
  }

  .row {
    display: grid;
    grid-template-columns: 28px 120px 120px 72px 1fr auto auto auto;
    gap: 10px;
    align-items: center;
  }

  .frames {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin: 0 0 8px 38px;
  }

  .frame-edit {
    display: grid;
    gap: 6px;
  }

  .nudge {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .nudge .dim {
    flex: 1;
    font-size: 13px;
  }

  .row.header {
    font-size: 13px;
  }

  .row-error {
    margin: -4px 0 0 38px;
    font-size: 13px;
  }

  p {
    margin: 10px 0 0;
  }

  .footer {
    display: flex;
    flex-wrap: wrap;
    gap: 14px 20px;
    align-items: center;
  }

  .format {
    display: flex;
    gap: 6px;
  }

  .toggle {
    display: flex;
    gap: 8px;
    align-items: center;
    white-space: nowrap;
    cursor: pointer;
  }

  .out {
    flex: 1;
    min-width: 260px;
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
</style>
