<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { api, type Tool, type ToolInfo, type ToolProgress } from "$lib/api";

  let { tools, onready }: { tools: ToolInfo[]; onready: () => void } = $props();

  let installing = $state(false);
  let error = $state<string | null>(null);
  let progress = $state<Partial<Record<Tool, ToolProgress>>>({});

  // ffprobe comes in the ffmpeg archive, so it is one download unit.
  const units = $derived([
    ...new Set(tools.filter((t) => !t.path).map((t) => (t.tool === "ffprobe" ? "ffmpeg" : t.tool))),
  ] as Tool[]);

  onMount(() => {
    const unlisten = listen<ToolProgress>("tools-progress", (e) => {
      progress[e.payload.tool] = e.payload;
    });
    return () => unlisten.then((f) => f());
  });

  async function install() {
    installing = true;
    error = null;
    progress = {};
    try {
      const status = await api.toolsInstall();
      if (status.every((t) => t.path)) onready();
    } catch (e) {
      error = String(e);
    } finally {
      installing = false;
    }
  }

  const mb = (bytes: number) => (bytes / 1e6).toFixed(0);
  // Rough download sizes, MB.
  const estimate = $derived(units.reduce((sum, u) => sum + (u === "ffmpeg" ? 200 : 40), 0));
</script>

<main>
  <section class="glass card setup">
    <img src="/logo.svg" width="96" height="96" alt="WClip" />
    <h1>WClip</h1>
    <p class="dim">Для работы нужны ffmpeg и yt-dlp. WClip скачает недостающее один раз, около {estimate} МБ.</p>

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

    {#if error}
      <p class="error message">{error}</p>
    {/if}

    <button class="btn primary" onclick={install} disabled={installing}>
      {installing ? "Скачиваем…" : error ? "Повторить" : "Скачать"}
    </button>
  </section>
</main>

<style>
  main {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: 24px;
  }

  .setup {
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
  }

  .units {
    list-style: none;
    margin: 24px 0;
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

  .message {
    margin-bottom: 16px;
    word-break: break-word;
  }
</style>
