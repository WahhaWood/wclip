<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import { api, type Corner, type Settings } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app } from "$lib/state.svelte";
  import { parseTime } from "$lib/time";

  // The parent renders this screen only once settings are loaded.
  const s = $derived(app.settings as Settings);

  let fonts = $state<string[]>([]);
  let preview = $state<string | null>(null);
  let previewError = $state<string | null>(null);
  let previewing = $state(false);
  let presetName = $state("");

  const corners: [Corner, string][] = [
    ["top-left", "↖"],
    ["top-right", "↗"],
    ["bottom-left", "↙"],
    ["bottom-right", "↘"],
  ];

  // Preview frame: the first interval start of the open VOD, else its middle.
  const previewTime = $derived.by(() => {
    if (!app.vod) return 0;
    const first = app.rows.map((r) => parseTime(r.start)).find((t) => t !== null);
    return first ?? Math.floor(app.vod.duration / 2);
  });

  onMount(() => {
    api.fontFamilies().then((f) => (fonts = f), console.error);
    return () => {
      if (preview) URL.revokeObjectURL(preview);
    };
  });

  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  $effect(() => {
    const style = $state.snapshot(s.overlay);
    const playlist = app.vod?.playlist ?? null;
    const time = previewTime;
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const mine = ++seq;
      previewing = true;
      try {
        const png = await api.overlayPreview(playlist, time, style);
        if (mine !== seq) return;
        if (preview) URL.revokeObjectURL(preview);
        preview = URL.createObjectURL(new Blob([png], { type: "image/png" }));
        previewError = null;
      } catch (e) {
        if (mine === seq) previewError = explain(e).text;
      } finally {
        if (mine === seq) previewing = false;
      }
    }, 200);
  });

  function savePreset() {
    const name = presetName.trim();
    if (!name) return;
    const style = $state.snapshot(s.overlay);
    const existing = s.presets.find((p) => p.name === name);
    if (existing) existing.style = style;
    else s.presets.push({ name, style });
    presetName = "";
  }

  let logEmpty = $state(false);

  /** Shows the error log in its folder, so it can be attached to a bug report. */
  async function showLog() {
    const path = await api.errorLog();
    logEmpty = !path;
    if (path) await revealItemInDir(path);
  }

  async function chooseDir() {
    const dir = await open({ directory: true, defaultPath: s.outDir ?? undefined });
    if (typeof dir === "string") s.outDir = dir;
  }
</script>

<div class="screen">
  <section class="glass card">
    <h2>Оверлей чата</h2>
    <div class="overlay">
      <div class="preview">
        {#if preview}
          <img src={preview} alt="Превью оверлея" class:stale={previewing} />
        {:else}
          <div class="placeholder dim">{previewError ?? "Готовим превью…"}</div>
        {/if}
        <p class="dim hint">
          {app.vod
            ? "Кадр из открытой записи, сообщения для примера."
            : "Откройте запись, чтобы увидеть превью на её кадре."}
        </p>
        {#if preview && previewError}
          <p class="error hint">{previewError}</p>
        {/if}
      </div>

      <div class="controls">
        <div class="control">
          <span>Угол</span>
          <div class="corners">
            {#each corners as [corner, arrow] (corner)}
              <button
                class="btn small"
                class:selected={s.overlay.corner === corner}
                onclick={() => (s.overlay.corner = corner)}>{arrow}</button
              >
            {/each}
          </div>
        </div>

        <label class="control">
          <span>Ширина <b>{s.overlay.width}</b></span>
          <input type="range" min="200" max="1000" step="10" bind:value={s.overlay.width} />
        </label>
        <label class="control">
          <span>Высота <b>{s.overlay.height}</b></span>
          <input type="range" min="150" max="1000" step="10" bind:value={s.overlay.height} />
        </label>
        <label class="control">
          <span>Отступ от края <b>{s.overlay.margin}</b></span>
          <input type="range" min="0" max="200" step="2" bind:value={s.overlay.margin} />
        </label>
        <label class="control">
          <span>Шрифт</span>
          <select class="field" bind:value={s.overlay.font}>
            {#if !fonts.includes(s.overlay.font)}
              <option value={s.overlay.font}>{s.overlay.font}</option>
            {/if}
            {#each fonts as font (font)}
              <option value={font}>{font}</option>
            {/each}
          </select>
        </label>
        <label class="control">
          <span>Размер текста <b>{s.overlay.fontSize}</b></span>
          <input type="range" min="12" max="48" step="1" bind:value={s.overlay.fontSize} />
        </label>
        <label class="control">
          <span>Фон <b>{Math.round(s.overlay.background * 100)}%</b></span>
          <input type="range" min="0" max="1" step="0.05" bind:value={s.overlay.background} />
        </label>
        <label class="control">
          <span>
            Время жизни сообщения
            <b>{s.overlay.lifetime > 0 ? `${s.overlay.lifetime} с` : "не исчезают"}</b>
          </span>
          <input type="range" min="0" max="120" step="1" bind:value={s.overlay.lifetime} />
        </label>
        <p class="dim hint">Размеры в пикселях для 1080p, на другом разрешении масштабируются.</p>
      </div>
    </div>
  </section>

  <section class="glass card">
    <h2>Пресеты</h2>
    <div class="presets">
      {#each s.presets as preset, i (preset.name)}
        <div class="preset">
          <button class="btn small" onclick={() => (s.overlay = $state.snapshot(preset.style))}>
            {preset.name}
          </button>
          <button class="btn small" title="Удалить" onclick={() => s.presets.splice(i, 1)}>✕</button>
        </div>
      {:else}
        <span class="dim">Пока нет. Сохраните текущие настройки оверлея под именем.</span>
      {/each}
    </div>
    <form
      class="line"
      onsubmit={(e) => {
        e.preventDefault();
        savePreset();
      }}
    >
      <input class="field" placeholder="Имя пресета" bind:value={presetName} />
      <button class="btn primary" disabled={!presetName.trim()}>Сохранить</button>
    </form>
  </section>

  <section class="glass card">
    <h2>Общие</h2>
    <div class="general">
      <span>Папка для клипов</span>
      <div class="line">
        <input class="field" readonly value={s.outDir ?? ""} />
        <button class="btn" onclick={chooseDir}>Выбрать…</button>
      </div>
      <span>Имя файла</span>
      <div>
        <input class="field" bind:value={s.nameTemplate} />
        <p class="dim hint">
          Если у интервала нет своего имени. Подстановки: {"{channel}"} — канал, {"{vod}"} — номер
          записи, {"{start}"} и {"{end}"} — время, {"{n}"} — номер клипа.
        </p>
      </div>
      <span>Журнал ошибок</span>
      <div>
        <button class="btn" onclick={showLog}>Показать файл</button>
        <p class="dim hint">
          {logEmpty ? "Ошибок пока не было." : "Приложите его к сообщению об ошибке. Ссылки в нём без токенов."}
        </p>
      </div>
    </div>
  </section>
</div>

<style>
  .screen {
    display: grid;
    gap: 16px;
  }

  .overlay {
    display: grid;
    grid-template-columns: minmax(0, 1.6fr) minmax(260px, 1fr);
    gap: 24px;
    align-items: start;
  }

  .preview img,
  .placeholder {
    width: 100%;
    aspect-ratio: 16 / 9;
    border-radius: 12px;
    border: 1px solid var(--outline);
  }

  .preview img.stale {
    opacity: 0.7;
  }

  .placeholder {
    display: grid;
    place-items: center;
    padding: 16px;
    text-align: center;
  }

  .controls {
    display: grid;
    gap: 14px;
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

  .corners {
    display: flex;
    gap: 6px;
  }

  input[type="range"] {
    width: 100%;
    accent-color: var(--primary);
  }

  .hint {
    margin: 8px 0 0;
    font-size: 13px;
  }

  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-bottom: 14px;
  }

  .preset {
    display: flex;
    gap: 4px;
  }

  .line {
    display: flex;
    gap: 10px;
  }

  .general {
    display: grid;
    grid-template-columns: 160px 1fr;
    gap: 14px 16px;
    align-items: start;
  }

  .general > span {
    padding-top: 9px;
  }
</style>
