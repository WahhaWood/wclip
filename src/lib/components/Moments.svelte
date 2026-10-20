<script lang="ts">
  import { api, type Activity, type ChatLogRef, type Peak, type Vod } from "$lib/api";
  import { explain } from "$lib/errors";
  import { addInterval, app, chatKey } from "$lib/state.svelte";
  import { formatTime } from "$lib/time";
  import Frame from "./Frame.svelte";

  let { vod, chatLog }: { vod: Vod; chatLog: ChatLogRef | null } = $props();

  /** Chat reacts late: a clip starts this long before the burst and ends a bit after it. */
  const LEAD = 20;
  const TAIL = 5;
  const SHOWN = 8;
  const CHART_H = 96;

  const key = $derived(chatKey(vod, chatLog));
  const status = $derived(app.chat.key === key ? app.chat.status : ({ state: "idle" } as const));

  let query = $state("");
  let activity = $state<Activity | null>(null);
  let activityError = $state<string | null>(null);
  let byTime = $state(false);
  let showAll = $state(false);
  /** Time picked on the chart or from a peak, shown as a frame. */
  let cursor = $state<number | null>(null);
  let hover = $state<number | null>(null);
  let chartWidth = $state(0);

  const clamp = (t: number) => Math.max(0, Math.min(t, vod.duration));
  const max = $derived(activity ? Math.max(1, ...activity.counts) : 1);
  const peaks = $derived.by(() => {
    if (!activity) return [];
    const list = byTime ? [...activity.peaks].sort((a, b) => a.start - b.start) : activity.peaks;
    return showAll ? list : list.slice(0, SHOWN);
  });
  /** Bars inside a peak, highlighted on the chart. */
  const inPeak = $derived.by(() => {
    if (!activity) return new Set<number>();
    const bar = activity.bar;
    return new Set(
      activity.peaks.flatMap((p) => {
        const out = [];
        for (let i = Math.floor(p.start / bar); i * bar < p.end; i++) out.push(i);
        return out;
      }),
    );
  });
  const ticks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => f * vod.duration));
  /** Interval start times already in the table, to mark peaks taken. */
  const taken = $derived(new Set(app.rows.map((r) => r.start.trim())));

  async function load() {
    if (!key) return;
    app.chat = { key, status: { state: "loading", fraction: 0 } };
    try {
      const count = await api.chatLoad($state.snapshot(vod), chatLog && $state.snapshot(chatLog));
      if (app.chat.key === key) app.chat.status = { state: "ready", count };
    } catch (e) {
      // A load for another video replaced this one.
      if (app.chat.key === key) app.chat.status = { state: "failed", error: e };
    }
  }

  // Recomputed a moment after the search stops changing.
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (status.state !== "ready") {
      activity = null;
      return;
    }
    const q = query;
    clearTimeout(timer);
    let stale = false;
    timer = setTimeout(() => {
      api.chatActivity(q).then(
        (a) => {
          if (stale) return;
          activity = a;
          activityError = null;
        },
        (e) => {
          if (!stale) activityError = explain(e).text;
        },
      );
    }, 250);
    return () => (stale = true);
  });

  const peakStart = (p: Peak) => clamp(p.start - LEAD);
  const peakEnd = (p: Peak) => clamp(p.end + TAIL);

  function barAt(e: MouseEvent) {
    if (!activity || !chartWidth) return null;
    const i = Math.floor((e.offsetX / chartWidth) * activity.counts.length);
    return Math.max(0, Math.min(i, activity.counts.length - 1));
  }

  function pick(e: MouseEvent) {
    const i = barAt(e);
    if (i !== null && activity) cursor = Math.round((i + 0.5) * activity.bar);
  }
</script>

{#if key}
  <section class="glass card">
    <div class="head">
      <h2>Моменты</h2>
      {#if status.state === "ready"}
        <span class="dim">{status.count.toLocaleString("ru")} сообщ. в чате</span>
      {/if}
    </div>

    {#if status.state === "idle" || status.state === "failed"}
      <div class="intro">
        <p class="dim">
          Чат всей записи покажет, где зрители реагировали сильнее всего. Загрузка занимает около минуты на
          4 часа записи.
        </p>
        <button class="btn primary" onclick={load}>
          {status.state === "failed" ? "Повторить" : "Загрузить чат"}
        </button>
      </div>
      {#if status.state === "failed"}
        {@const e = explain(status.error)}
        <p class="error" title={e.detail}>{e.text}</p>
      {/if}
    {:else if status.state === "loading"}
      <div class="loading">
        <span class="dim">Загружаем чат… {Math.round(status.fraction * 100)}%</span>
        <div class="bar"><div style:width="{status.fraction * 100}%"></div></div>
      </div>
    {:else}
      <input class="field search" placeholder="Поиск по чату: KEKW, LUL, клип — через запятую" bind:value={query} />
      {#if activityError}
        <p class="error">{activityError}</p>
      {/if}

      {#if activity}
        <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
        <div
          class="chart"
          bind:clientWidth={chartWidth}
          onpointermove={(e) => (hover = barAt(e))}
          onpointerleave={() => (hover = null)}
          onclick={pick}
        >
          <svg viewBox="0 0 {activity.counts.length} {CHART_H}" preserveAspectRatio="none" height={CHART_H}>
            {#each activity.counts as c, i (i)}
              {#if c > 0}
                {@const h = Math.max(1, (c / max) * CHART_H)}
                <rect x={i} y={CHART_H - h} width="1" height={h} class:peak={inPeak.has(i)} class:hover={hover === i} />
              {/if}
            {/each}
            {#if cursor !== null}
              <rect x={cursor / activity.bar} y="0" width="0.6" height={CHART_H} class="cursor" />
            {/if}
          </svg>
        </div>
        <div class="axis dim">
          {#each ticks as t (t)}<span>{formatTime(Math.floor(t))}</span>{/each}
        </div>
        <p class="readout dim">
          {#if hover !== null}
            {formatTime(hover * activity.bar)} – {formatTime(Math.min((hover + 1) * activity.bar, vod.duration))} ·
            {activity.counts[hover]} сообщ.
          {:else}
            {query.trim() ? `Совпадений: ${activity.total.toLocaleString("ru")}` : "Нажмите на график, чтобы увидеть кадр"}
          {/if}
        </p>

        {#if cursor !== null}
          <div class="cursor-view">
            <Frame playlist={vod.playlist} time={cursor} />
            <div class="cursor-side">
              <div class="time">{formatTime(cursor)}</div>
              <div class="nudge">
                {#each [-10, -1, 1, 10] as d (d)}
                  <button class="btn small" onclick={() => (cursor = clamp(cursor! + d))}>
                    {d > 0 ? `+${d}` : d} с
                  </button>
                {/each}
              </div>
              <button class="btn small primary" onclick={() => addInterval(clamp(cursor! - LEAD), clamp(cursor! + 10))}>
                + Интервал вокруг
              </button>
              <button class="btn small" onclick={() => (cursor = null)}>Скрыть</button>
            </div>
          </div>
        {/if}

        {#if activity.peaks.length}
          <div class="peaks-head">
            <span class="dim">Всплески чата</span>
            <div class="sort">
              <button class="btn small" class:selected={!byTime} onclick={() => (byTime = false)}>Сильные</button>
              <button class="btn small" class:selected={byTime} onclick={() => (byTime = true)}>По времени</button>
            </div>
          </div>
          <div class="peaks">
            {#each peaks as p (p.start)}
              {@const start = formatTime(Math.round(peakStart(p)))}
              <div class="peak">
                <span class="when">{formatTime(p.start)}</span>
                <span class="dim count">{p.count} сообщ.</span>
                <span class="words">
                  {#each p.words as [w, n] (w)}<span class="chip">{w} <span class="dim">×{n}</span></span>{/each}
                </span>
                <button class="btn small" onclick={() => (cursor = p.start)}>Кадр</button>
                <button
                  class="btn small"
                  disabled={taken.has(start)}
                  onclick={() => addInterval(peakStart(p), peakEnd(p))}
                >
                  {taken.has(start) ? "Добавлен" : "+ Интервал"}
                </button>
              </div>
            {/each}
          </div>
          {#if activity.peaks.length > SHOWN}
            <button class="btn small more" onclick={() => (showAll = !showAll)}>
              {showAll ? "Свернуть" : `Все всплески (${activity.peaks.length})`}
            </button>
          {/if}
        {:else}
          <p class="dim">Заметных всплесков нет.</p>
        {/if}
      {/if}
    {/if}
  </section>
{/if}

<style>
  .head,
  .peaks-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }

  .intro {
    display: flex;
    gap: 16px;
    align-items: center;
    justify-content: space-between;
  }

  .intro p {
    margin: 0;
  }

  .loading {
    display: grid;
    gap: 8px;
  }

  .search {
    width: 100%;
    box-sizing: border-box;
  }

  .chart {
    margin-top: 14px;
    cursor: crosshair;
  }

  svg {
    display: block;
    width: 100%;
  }

  rect {
    fill: var(--outline);
  }

  rect.peak {
    fill: var(--primary);
  }

  rect.hover {
    fill: var(--tertiary);
  }

  rect.cursor {
    fill: var(--text);
  }

  .axis {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    margin-top: 4px;
  }

  .readout {
    font-size: 13px;
    min-height: 1.4em;
  }

  .cursor-view {
    display: grid;
    grid-template-columns: minmax(0, 480px) auto;
    gap: 16px;
    margin-top: 12px;
  }

  .cursor-side {
    display: grid;
    gap: 8px;
    align-content: start;
  }

  .time {
    font-size: 20px;
    font-weight: 600;
  }

  .nudge,
  .sort {
    display: flex;
    gap: 6px;
  }

  .peaks-head {
    margin-top: 16px;
  }

  .peaks {
    display: grid;
    gap: 6px;
    margin-top: 8px;
  }

  .peak {
    display: grid;
    grid-template-columns: 72px 92px minmax(0, 1fr) auto auto;
    gap: 10px;
    align-items: center;
  }

  .when {
    font-weight: 600;
  }

  .count {
    font-size: 13px;
  }

  .words {
    display: flex;
    gap: 6px;
    overflow: hidden;
  }

  .chip {
    padding: 2px 8px;
    border-radius: 8px;
    background: var(--surface-highest);
    font-size: 13px;
    white-space: nowrap;
  }

  .more {
    margin-top: 10px;
  }

  p {
    margin: 10px 0 0;
  }
</style>
