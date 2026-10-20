<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { api, type Interval, type Settings } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app, type Row } from "$lib/state.svelte";

  /** `interval` is the row's, as checked. */
  let { row, interval }: { row: Row; interval: Interval } = $props();

  const s = $derived(app.settings as Settings);
  const key = $derived(`${interval.start}|${interval.end}`);
  const current = $derived(row.captions && row.captionsFor === key ? row.captions : null);

  let percent = $state<number | null>(null);
  let error = $state<ReturnType<typeof explain> | null>(null);

  onMount(() => {
    if (app.speechModel === null) api.speechModelReady().then((ready) => (app.speechModel = ready), console.error);
    const unlisten = listen<number>("transcribe-progress", (e) => {
      if (percent !== null) percent = e.payload;
    });
    return () => unlisten.then((f) => f());
  });

  async function run() {
    if (!app.vod) return;
    error = null;
    percent = 0;
    try {
      const captions = await api.transcribe(
        $state.snapshot(app.vod),
        interval.start,
        interval.end,
        s.speechLanguage,
        s.subtitleStyle.words,
      );
      row.captions = captions;
      row.captionsFor = key;
    } catch (e) {
      if (String((e as { detail?: string }).detail ?? e) !== "cancelled") error = explain(e);
    } finally {
      percent = null;
    }
  }

  /** `m:ss.s` from the clip start. */
  function stamp(t: number) {
    const m = Math.floor(t / 60);
    return `${m}:${(t - m * 60).toFixed(1).padStart(4, "0")}`;
  }
</script>

<div class="captions">
  {#if app.speechModel === false}
    <p class="dim">
      Для субтитров нужна модель распознавания речи.
      <button class="btn small" onclick={() => (app.screen = "settings")}>Скачать в настройках</button>
    </p>
  {:else}
    <div class="line">
      <button class="btn small" class:primary={!current} disabled={percent !== null} onclick={run}>
        {percent !== null ? `Распознаём… ${percent}%` : current ? "Распознать заново" : "Распознать речь"}
      </button>
      {#if current}
        <button class="btn small" disabled={percent !== null} onclick={() => (row.captions = row.captionsFor = null)}>
          Сбросить правки
        </button>
      {/if}
      <span class="dim">
        {#if row.captions && !current}
          Интервал изменился, текст устарел: распознайте заново.
        {:else if !current}
          Без правки речь распознается сама при нарезке.
        {/if}
      </span>
    </div>
    {#if error}
      <p class="error" title={error.detail}>{error.text}</p>
    {/if}
    {#if current}
      {#if current.length === 0}
        <p class="dim">Речи не найдено.</p>
      {/if}
      <div class="list">
        {#each current as caption, i (i)}
          <span class="dim time">{stamp(caption.start)}</span>
          <input class="field" bind:value={caption.text} />
          <button class="btn small" title="Убрать" onclick={() => current.splice(i, 1)}>✕</button>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .captions {
    display: grid;
    gap: 10px;
    margin: 0 0 8px 38px;
  }

  .captions p {
    margin: 0;
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    font-size: 14px;
  }

  .list {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 6px 10px;
    align-items: center;
    max-height: 360px;
    overflow-y: auto;
  }

  .time {
    font-variant-numeric: tabular-nums;
    font-size: 13px;
  }
</style>
