<script lang="ts">
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { api } from "$lib/api";
  import { app } from "$lib/state.svelte";
  import { formatTime } from "$lib/time";

  const done = $derived(app.queue.filter((q) => q.status.state === "done").length);
  const lastPath = $derived(
    app.queue.findLast((q) => q.status.state === "done")?.status as { path: string } | undefined,
  );

  async function cancel() {
    await api.jobCancel();
    app.running = false;
    for (const q of app.queue) {
      if (q.status.state !== "done" && q.status.state !== "failed") {
        q.status = { state: "failed", error: "Отменено" };
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
      <li>
        <div class="row">
          <span class="name">{item.name}</span>
          <span class="dim">{formatTime(item.start)} – {formatTime(item.end)}</span>
          <span class="state" class:error={s.state === "failed"} title={s.state === "failed" ? s.error : undefined}>
            {#if s.state === "waiting"}
              <span class="dim">Ожидает</span>
            {:else if s.state === "preparing"}
              <span class="dim">Загружаем чат…</span>
            {:else if s.state === "running"}
              {Math.round(s.fraction * 100)}%
            {:else if s.state === "done"}
              Готово
            {:else}
              {s.error}
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
  </ul>

  <div class="actions">
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

  .actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
    margin-top: 24px;
  }
</style>
