<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Tool = "ffmpeg" | "ffprobe" | "yt-dlp";
  type ToolInfo = { tool: Tool; path: string | null; version: string | null };
  type Progress = { tool: Tool; done: number; total: number | null };

  let tools = $state<ToolInfo[] | null>(null);
  let installing = $state(false);
  let error = $state<string | null>(null);
  let progress = $state<Partial<Record<Tool, Progress>>>({});

  const missing = $derived(tools?.filter((t) => !t.path) ?? []);
  // ffprobe comes in the ffmpeg archive, so it is one download unit.
  const units = $derived(
    [...new Set(missing.map((t) => (t.tool === "ffprobe" ? "ffmpeg" : t.tool)))] as Tool[],
  );

  onMount(() => {
    const unlisten = listen<Progress>("tools-progress", (e) => {
      progress[e.payload.tool] = e.payload;
    });
    invoke<ToolInfo[]>("tools_status")
      .then((s) => (tools = s))
      .catch((e) => (error = String(e)));
    return () => unlisten.then((f) => f());
  });

  async function install() {
    installing = true;
    error = null;
    progress = {};
    try {
      tools = await invoke<ToolInfo[]>("tools_install");
    } catch (e) {
      error = String(e);
    } finally {
      installing = false;
    }
  }

  const mb = (bytes: number) => (bytes / 1e6).toFixed(0);
</script>

<main>
  <section class="glass card">
    <img src="/logo.svg" width="96" height="96" alt="WClip" />
    <h1>WClip</h1>

    {#if !tools && !error}
      <p>Проверяем инструменты…</p>
    {:else if tools && missing.length === 0}
      <p>Нарезка записей Twitch с чатом в кадре.</p>
    {:else}
      <p>Для работы нужны ffmpeg и yt-dlp. WClip скачает их один раз, около 200 МБ.</p>

      {#if tools}
        <ul class="units">
          {#each units as unit (unit)}
            {@const p = progress[unit]}
            <li>
              <div class="row">
                <span>{unit === "ffmpeg" ? "ffmpeg + ffprobe" : unit}</span>
                <span class="dim">
                  {#if p?.total}{mb(p.done)} / {mb(p.total)} МБ{:else if p}{mb(p.done)} МБ{/if}
                </span>
              </div>
              <div class="bar">
                <div style:width="{p?.total ? (100 * p.done) / p.total : 0}%"></div>
              </div>
            </li>
          {/each}
        </ul>
      {/if}

      {#if error}
        <p class="error">{error}</p>
      {/if}

      {#if tools}
        <button onclick={install} disabled={installing}>
          {installing ? "Скачиваем…" : error ? "Повторить" : "Скачать"}
        </button>
      {/if}
    {/if}
  </section>
</main>

<style>
  main {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: 24px;
  }

  .card {
    width: min(440px, 100%);
    padding: 40px 48px;
    text-align: center;
  }

  h1 {
    margin: 16px 0 8px;
    font-size: 32px;
    font-weight: 700;
  }

  p {
    margin: 0;
    color: var(--text-dim);
  }

  .units {
    list-style: none;
    margin: 24px 0 0;
    padding: 0;
    display: grid;
    gap: 14px;
    text-align: left;
  }

  .row {
    display: flex;
    justify-content: space-between;
    margin-bottom: 6px;
    font-size: 14px;
  }

  .dim {
    color: var(--text-dim);
  }

  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--glass);
    overflow: hidden;
  }

  .bar > div {
    height: 100%;
    background: linear-gradient(90deg, var(--glow-cyan), var(--glow-violet));
    transition: width 0.2s;
  }

  .error {
    margin-top: 16px;
    color: var(--danger);
    word-break: break-word;
  }

  button {
    margin-top: 24px;
    padding: 12px 32px;
    border: 1px solid var(--glass-border);
    border-radius: 999px;
    background: color-mix(in srgb, var(--seed) 24%, transparent);
    color: var(--text);
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
