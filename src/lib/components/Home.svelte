<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type Interval } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app, newRow, runJob, type Row } from "$lib/state.svelte";
  import { formatTime, parseIntervalList, parseTime } from "$lib/time";

  let link = $state("");
  let loading = $state(false);
  let error = $state<ReturnType<typeof explain> | null>(null);
  let starting = $state(false);
  let pasteOpen = $state(false);
  let pasteText = $state("");
  let pasteBad = $state<string[]>([]);

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
  const ready = $derived(
    !!app.vod && intervals.length > 0 && checks.every((c) => !c.error) && !app.running,
  );

  async function openVod() {
    if (!link.trim()) return;
    loading = true;
    error = null;
    try {
      app.vod = await api.vodInfo(link);
      if (app.rows.length === 0) app.rows = [newRow()];
    } catch (e) {
      error = explain(e);
    } finally {
      loading = false;
    }
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

  async function start() {
    const s = app.settings;
    if (!app.vod || !s) return;
    starting = true;
    error = null;
    try {
      app.queueVod = app.vod;
      app.queue = intervals.map((interval) => ({ interval, status: { state: "waiting" } }));
      await runJob(
        intervals.map((_, i) => i),
        s.chat ? $state.snapshot(s.overlay) : null,
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
  <section class="glass card">
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
    </form>
    {#if error}
      <p class="error" title={error.detail}>{error.text}</p>
    {/if}

    {#if app.vod}
      <div class="vod">
        {#if app.vod.thumbnail}
          <img src={app.vod.thumbnail} alt="" />
        {/if}
        <div>
          <div class="title">{app.vod.title}</div>
          <div class="dim">{app.vod.channel} · {formatTime(app.vod.duration)}</div>
        </div>
      </div>
    {/if}
  </section>

  {#if app.vod}
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
          <span>№</span><span>Начало</span><span>Конец</span><span>Длина</span><span>Имя клипа</span><span></span>
        </div>
        {#each app.rows as row, i (row.id)}
          {@const c = checks[i]}
          <div class="row">
            <span class="dim">{i + 1}</span>
            <input class="field" class:invalid={c?.field === "start"} placeholder="0:00:00" bind:value={row.start} />
            <input class="field" class:invalid={c?.field === "end"} placeholder="0:00:30" bind:value={row.end} />
            <span class="dim">{c?.interval ? formatTime(c.interval.end - c.interval.start) : "—"}</span>
            <input class="field" placeholder="необязательно" bind:value={row.name} />
            <button class="btn small" title="Удалить" onclick={() => app.rows.splice(i, 1)}>✕</button>
          </div>
          {#if c?.error}
            <p class="error row-error">{c.error}</p>
          {/if}
        {/each}
      </div>
    </section>

    {#if app.settings}
      <section class="glass card footer">
        <label class="toggle">
          <input type="checkbox" bind:checked={app.settings.chat} />
          Чат в кадре
          {#if app.settings.chat && app.settings.activePreset}
            <span class="dim">· {app.settings.activePreset}</span>
          {/if}
        </label>
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
    grid-template-columns: 28px 120px 120px 72px 1fr auto;
    gap: 10px;
    align-items: center;
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
    gap: 20px;
    align-items: center;
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
    min-width: 0;
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
