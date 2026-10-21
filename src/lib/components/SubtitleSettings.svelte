<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { api, type Settings } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app } from "$lib/state.svelte";
  import SubtitleBox from "./SubtitleBox.svelte";

  /** A frame of the open video for the preview. */
  let { frame }: { frame: string | null } = $props();

  // The parent renders this only once settings are loaded.
  const s = $derived(app.settings as Settings);
  const style = $derived(s.subtitleStyle);

  const LANGUAGES: [string | null, string][] = [
    [null, "Определять самому"],
    ["ru", "Русский"],
    ["en", "English"],
    ["uk", "Українська"],
    ["de", "Deutsch"],
    ["es", "Español"],
    ["fr", "Français"],
  ];
  const MODEL_MB = 874;

  /** Bytes done and total of the model download; `null` when not downloading. */
  let progress = $state<{ done: number; total: number | null } | null>(null);
  let error = $state<ReturnType<typeof explain> | null>(null);

  onMount(() => {
    if (app.speechModel === null) api.speechModelReady().then((ready) => (app.speechModel = ready), console.error);
    const unlisten = listen<{ done: number; total: number | null }>("model-progress", (e) => (progress = e.payload));
    return () => unlisten.then((f) => f());
  });

  async function install() {
    error = null;
    progress = { done: 0, total: null };
    try {
      await api.speechModelInstall();
      app.speechModel = true;
    } catch (e) {
      error = explain(e);
    } finally {
      progress = null;
    }
  }

  const hex = (c: [number, number, number]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
  function color(value: string): [number, number, number] {
    const n = parseInt(value.slice(1), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }

  let previewHeight = $state(0);
  const k = $derived(previewHeight / 1080);
</script>

<section class="glass card">
  <h2>Субтитры</h2>
  <div class="general">
    <span>Распознавание речи</span>
    <div>
      {#if app.speechModel}
        <p class="state">Модель скачана.</p>
      {:else if progress}
        <p class="state">
          Скачиваем модель… {progress.total ? `${Math.round((progress.done / progress.total) * 100)}%` : ""}
        </p>
        <div class="bar"><div style:width="{progress.total ? (progress.done / progress.total) * 100 : 0}%"></div></div>
      {:else}
        <button class="btn primary" onclick={install}>Скачать модель ({MODEL_MB} МБ)</button>
      {/if}
      {#if error}
        <p class="error hint" title={error.detail}>{error.text}</p>
      {/if}
      <p class="dim hint">
        Whisper large-v3-turbo работает на этом компьютере, без интернета. Минута речи распознаётся примерно за минуту на
        процессоре. Ошибки в именах и сленге можно поправить у интервала (кнопка «Текст»).
      </p>
    </div>

    <span>Язык речи</span>
    <select class="field" bind:value={s.speechLanguage}>
      {#each LANGUAGES as [code, label] (code)}
        <option value={code}>{label}</option>
      {/each}
    </select>

    <span>Вид</span>
    <div class="controls">
      <div class="preview" bind:clientHeight={previewHeight}>
        {#if frame}
          <img src={frame} alt="" draggable="false" />
        {/if}
        <SubtitleBox bind:x={style.x} bind:y={style.y} {k} />
      </div>
      <label class="control">
        <span>Размер <b>{style.size}</b></span>
        <input type="range" min="32" max="140" step="2" bind:value={style.size} />
      </label>
      <label class="control">
        <span>Обводка <b>{style.outline}</b></span>
        <input type="range" min="0" max="12" step="1" bind:value={style.outline} />
      </label>
      <label class="control">
        <span>Слов в титре <b>{style.words}</b></span>
        <input type="range" min="1" max="8" step="1" bind:value={style.words} />
      </label>
      <div class="line">
        <label class="line">
          <input type="color" value={hex(style.color)} oninput={(e) => (style.color = color(e.currentTarget.value))} />
          Текст
        </label>
        <label class="line">
          <input
            type="color"
            value={hex(style.highlight)}
            oninput={(e) => (style.highlight = color(e.currentTarget.value))}
          />
          Текущее слово
        </label>
      </div>
      <label class="line">
        <input type="checkbox" bind:checked={style.uppercase} />
        ВЕРХНИЙ РЕГИСТР
      </label>
    </div>
  </div>
  <p class="dim hint">
    Тяните субтитры на превью, чтобы поставить их в кадре 16:9; на превью — первый поправленный титр интервалов, если
    он есть. В вертикальном видео место субтитров задаётся в раскладке. Шрифт — Inter Bold, как в превью.
  </p>
</section>

<style>
  .general {
    display: grid;
    grid-template-columns: 160px 1fr;
    gap: 14px 16px;
    align-items: start;
  }

  .general > span {
    padding-top: 9px;
  }

  .state {
    margin: 9px 0 0;
  }

  .hint {
    margin: 8px 0 0;
    font-size: 13px;
  }

  .bar {
    height: 4px;
    margin-top: 8px;
    border-radius: 2px;
    background: var(--surface-highest);
    overflow: hidden;
  }

  .bar div {
    height: 100%;
    background: var(--primary);
  }

  .controls {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 14px 24px;
    align-items: center;
  }

  .preview {
    grid-column: 1 / -1;
    position: relative;
    aspect-ratio: 16 / 9;
    max-width: 520px;
    border-radius: 12px;
    border: 1px solid var(--outline);
    background: linear-gradient(135deg, var(--surface-highest), var(--bg));
    overflow: hidden;
  }

  .preview img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    pointer-events: none;
  }

  .control {
    display: grid;
    gap: 6px;
    font-size: 14px;
  }

  .control b {
    float: right;
    font-weight: 600;
  }

  input[type="range"] {
    width: 100%;
    accent-color: var(--primary);
  }

  input[type="color"] {
    width: 36px;
    height: 30px;
    padding: 0;
    border: 1px solid var(--outline);
    border-radius: 8px;
    background: none;
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    font-size: 14px;
  }
</style>
