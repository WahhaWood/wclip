<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app, saveSettings } from "$lib/state.svelte";

  const u = $derived(app.update!);
  const st = $derived(app.updateState);
  const failure = $derived(st.state === "failed" ? explain(st.error) : null);

  async function install() {
    app.updateState = { state: "downloading", fraction: null };
    try {
      await api.updateInstall();
      app.updateState = { state: "ready" };
    } catch (e) {
      app.updateState = { state: "failed", error: e };
    }
  }

  async function restart() {
    try {
      await api.updateRestart();
    } catch (e) {
      app.updateState = { state: "failed", error: e };
    }
  }

  function skip() {
    if (app.settings) {
      app.settings.skippedVersion = u.version;
      saveSettings();
    }
    app.update = null;
  }
</script>

<section class="glass card">
  <h2>Доступна версия {u.version}</h2>
  {#if u.notes.trim()}
    <pre class="notes dim">{u.notes.trim()}</pre>
  {/if}

  {#if st.state === "downloading"}
    <div class="bar"><div style:width="{(st.fraction ?? 0) * 100}%"></div></div>
    <p class="dim">Скачиваем{st.fraction !== null ? ` · ${Math.round(st.fraction * 100)}%` : "…"}</p>
  {:else if failure}
    <p class="error" title={failure.detail}>{failure.text}</p>
  {/if}

  <div class="actions">
    {#if st.state === "ready"}
      {#if app.running}
        <span class="dim">Перезапуск — после нарезки</span>
      {/if}
      <button class="btn primary" disabled={app.running} onclick={restart}>Перезапустить</button>
    {:else if st.state !== "downloading"}
      <button class="btn" onclick={skip}>Пропустить версию</button>
      <button class="btn" onclick={() => (app.update = null)}>Позже</button>
      {#if u.installable}
        <button class="btn primary" onclick={install}>{st.state === "failed" ? "Повторить" : "Обновить"}</button>
      {:else}
        <button class="btn primary" onclick={() => openUrl(u.page)}>Открыть страницу релиза</button>
      {/if}
    {/if}
  </div>
</section>

<style>
  section {
    margin-bottom: 16px;
  }

  .notes {
    margin: 0 0 12px;
    max-height: 160px;
    overflow: auto;
    white-space: pre-wrap;
    font: inherit;
    font-size: 14px;
  }

  .actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
    align-items: center;
    margin-top: 12px;
  }
</style>
