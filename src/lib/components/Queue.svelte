<script lang="ts">
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { api } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app, runJob } from "$lib/state.svelte";
  import { formatTime } from "$lib/time";

  const done = $derived(app.queue.filter((q) => q.status.state === "done").length);
  const lastPath = $derived(
    app.queue.findLast((q) => q.status.state === "done")?.status as { path: string } | undefined,
  );

  /** Clips that failed only because the chat did not load. */
  const chatFailed = $derived(
    app.queue.flatMap((q, i) => (q.status.state === "failed" && q.status.error.kind === "chat" ? [i] : [])),
  );
  let error = $state<ReturnType<typeof explain> | null>(null);

  async function withoutChat() {
    error = null;
    try {
      await runJob(chatFailed, null);
    } catch (e) {
      error = explain(e);
    }
  }

  async function cancel() {
    await api.jobCancel();
    app.running = false;
    for (const q of app.queue) {
      if (q.status.state !== "done" && q.status.state !== "failed") {
        q.status = { state: "failed", error: { kind: "other", detail: "Отменено" } };
      }
    }
  }
</script>

<section class="glass card">
  <div class="head">
    <h2>Очередь</h2>
    <span class="dim">Готово {done} из {app.queue.length}</span>
  </div>

  <ul>
    {#each app.queue as item, i (i)}
      {@const s = item.status}
      {@const { start, end, name } = item.interval}
      {@const failure = s.state === "failed" ? explain(s.error) : null}
      <li>
        <div class="row">
          <span class="name">{name || `${formatTime(start)} – ${formatTime(end)}`}</span>
          <span class="dim">{formatTime(start)} – {formatTime(end)}</span>
          <span class="state" class:error={!!failure} title={failure ? (failure.detail ?? failure.text) : undefined}>
            {#if s.state === "waiting"}
              <span class="dim">Ожидает</span>
            {:else if s.state === "preparing"}
              <span class="dim">Загружаем чат…</span>
            {:else if s.state === "running"}
              {Math.round(s.fraction * 100)}%
            {:else if s.state === "retrying"}
              <span class="dim">Связь прервалась, повтор {s.attempt}…</span>
            {:else if s.state === "done"}
              Готово{#if s.noChat}<span class="dim"> · без чата: в отрезке нет сообщений</span>{/if}
            {:else}
              {failure?.text}
            {/if}
          </span>
        </div>
        <div class="bar" class:done={s.state === "done"} class:failed={s.state === "failed"}>
          <div
            style:width="{s.state === 'running' ? s.fraction * 100 : s.state === 'done' || s.state === 'failed' ? 100 : 0}%"
          ></div>
        </div>
      </li>
    {/each}
    {#if app.merge}
      {@const m = app.merge}
      {@const failure = m.state === "failed" ? explain(m.error) : null}
      <li>
        <div class="row">
          <span class="name">Все клипы одним роликом</span>
          <span></span>
          <span class="state" class:error={!!failure} title={failure?.detail}>
            {#if m.state === "running"}
              <span class="dim">Склеиваем…</span>
            {:else if m.state === "done"}
              Готово
            {:else}
              {failure?.text}
            {/if}
          </span>
        </div>
        <div class="bar" class:done={m.state === "done"} class:failed={m.state === "failed"}>
          <div style:width="{m.state === 'running' ? 50 : 100}%"></div>
        </div>
      </li>
    {/if}
  </ul>

  {#if error}
    <p class="error" title={error.detail}>{error.text}</p>
  {/if}

  <div class="actions">
    {#if !app.running && chatFailed.length > 0}
      <button class="btn" onclick={withoutChat}>Нарезать без чата ({chatFailed.length})</button>
    {/if}
    {#if app.running}
      <button class="btn" onclick={cancel}>Отменить</button>
    {:else}
      <button class="btn" onclick={() => (app.screen = "home")}>К интервалам</button>
    {/if}
    <button class="btn primary" disabled={!lastPath} onclick={() => lastPath && revealItemInDir(lastPath.path)}>
      Открыть папку
    </button>
  </div>
</section>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 16px;
  }

  .row {
    display: grid;
    grid-template-columns: 1fr auto minmax(80px, auto);
    gap: 16px;
    margin-bottom: 6px;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .state {
    text-align: right;
    max-width: 380px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .state.error {
    white-space: normal;
  }

  .actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
    margin-top: 24px;
  }
</style>
