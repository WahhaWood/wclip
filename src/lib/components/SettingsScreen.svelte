<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { getVersion } from "@tauri-apps/api/app";
  import { onMount } from "svelte";
  import { api, type ChatOutput, type Encoder, type Preset, type Quality, type Settings } from "$lib/api";
  import { explain } from "$lib/errors";
  import { app, showUpdate } from "$lib/state.svelte";
  import { parseTime } from "$lib/time";
  import SubtitleSettings from "./SubtitleSettings.svelte";

  // The parent renders this screen only once settings are loaded.
  const s = $derived(app.settings as Settings);

  let fonts = $state<string[]>([]);
  let frame = $state<string | null>(null);
  let frameError = $state<string | null>(null);
  let frameLoading = $state(false);
  let panel = $state<string | null>(null);
  let panelError = $state<string | null>(null);
  let presetName = $state("");
  let showToken = $state(false);

  const GPUS: Encoder[] = ["nvenc", "qsv", "amf", "vaapi"];
  const ENCODERS: Record<Encoder, string> = {
    auto: "Авто",
    cpu: "Процессор",
    nvenc: "NVIDIA NVENC",
    qsv: "Intel Quick Sync",
    amf: "AMD AMF",
    vaapi: "VAAPI (Linux)",
  };
  const QUALITIES: [Quality, string][] = [
    ["high", "Высокое"],
    ["medium", "Среднее"],
    ["small", "Компактное"],
  ];
  const CHAT_OUTPUTS: [ChatOutput, string][] = [
    ["burn", "В кадре"],
    ["file", "Отдельным файлом"],
    ["both", "И то и другое"],
  ];
  /** Working GPU encoders for the chosen codec; `null` while trying them. */
  let gpus = $state<Encoder[] | null>(null);
  $effect(() => {
    const codec = s.output.codec;
    gpus = null;
    api.encoders(codec).then((list) => {
      if (s.output.codec === codec) gpus = list;
    }, console.error);
  });

  /** Overlay sizes are pixels at this video height; the width follows the frame. */
  const H = 1080;
  const MIN_SIZE = 150;
  /** Distance from the edges for the corner buttons. */
  const EDGE = 24;
  let aspect = $state(16 / 9);
  const W = $derived(Math.round(H * aspect));
  let stageWidth = $state(0);
  const scale = $derived(stageWidth / W);

  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, hi));

  // Where ffmpeg puts the panel: moved back inside the frame when past an edge.
  const box = $derived({
    x: clamp(s.overlay.x, 0, W - s.overlay.width),
    y: clamp(s.overlay.y, 0, H - s.overlay.height),
    w: s.overlay.width,
    h: s.overlay.height,
  });

  const corners: [string, number, number][] = [
    ["↖", 0, 0],
    ["↗", 1, 0],
    ["↙", 0, 1],
    ["↘", 1, 1],
  ];

  function toCorner(right: number, bottom: number) {
    s.overlay.x = right ? W - s.overlay.width - EDGE : EDGE;
    s.overlay.y = bottom ? H - s.overlay.height - EDGE : EDGE;
  }

  // Preview frame: the first interval start of the open VOD, else its middle.
  const previewTime = $derived.by(() => {
    if (!app.vod) return 0;
    const first = app.rows.map((r) => parseTime(r.start)).find((t) => t !== null);
    return first ?? Math.floor(app.vod.duration / 2);
  });

  function blobUrl(old: string | null, bytes: ArrayBuffer, type: string) {
    if (old) URL.revokeObjectURL(old);
    return URL.createObjectURL(new Blob([bytes], { type }));
  }

  onMount(() => {
    api.fontFamilies().then((f) => (fonts = f), console.error);
    return () => {
      if (frame) URL.revokeObjectURL(frame);
      if (panel) URL.revokeObjectURL(panel);
    };
  });

  // The frame is fetched once per VOD and time; the panel goes over it separately.
  $effect(() => {
    const playlist = app.vod?.playlist;
    const time = previewTime;
    if (!playlist) return;
    let stale = false;
    frameLoading = true;
    api
      .previewFrame(playlist, time)
      .then(
        (jpeg) => {
          if (stale) return;
          frame = blobUrl(frame, jpeg, "image/jpeg");
          frameError = null;
        },
        (e) => {
          if (!stale) frameError = explain(e).text;
        },
      )
      .finally(() => {
        if (!stale) frameLoading = false;
      });
    return () => (stale = true);
  });

  // The panel depends on its look only; moving it needs no new picture.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  $effect(() => {
    const o = s.overlay;
    const look = {
      x: 0,
      y: 0,
      width: o.width,
      height: o.height,
      font: o.font,
      fontSize: o.fontSize,
      background: o.background,
      messageBackground: o.messageBackground,
      lifetime: o.lifetime,
    };
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const mine = ++seq;
      try {
        const png = await api.overlayPanel(look);
        if (mine !== seq) return;
        panel = blobUrl(panel, png, "image/png");
        panelError = null;
      } catch (e) {
        if (mine === seq) panelError = explain(e).text;
      }
    }, 60);
  });

  /** `move`, or the dragged corner as (x side, y side): 1 is the right or bottom edge. */
  type Drag = { mode: "move" | [number, number]; px: number; py: number; start: typeof box };
  let drag = $state<Drag | null>(null);

  function startDrag(e: PointerEvent, mode: Drag["mode"]) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    drag = { mode, px: e.clientX, py: e.clientY, start: { ...box } };
  }

  function onDrag(e: PointerEvent) {
    if (!drag || !scale) return;
    const dx = (e.clientX - drag.px) / scale;
    const dy = (e.clientY - drag.py) / scale;
    const { x, y, w, h } = drag.start;
    const o = s.overlay;
    if (drag.mode === "move") {
      o.x = Math.round(clamp(x + dx, 0, W - w));
      o.y = Math.round(clamp(y + dy, 0, H - h));
      return;
    }
    const [sx, sy] = drag.mode;
    [o.x, o.width] = resizeAxis(x, w, dx, sx, W);
    [o.y, o.height] = resizeAxis(y, h, dy, sy, H);
  }

  /** One axis of a corner drag: `far` moves the right or bottom edge, else the near one. */
  function resizeAxis(pos: number, size: number, d: number, far: number, limit: number): [number, number] {
    if (far) return [pos, Math.round(clamp(size + d, MIN_SIZE, limit - pos))];
    const end = pos + size;
    const start = Math.round(clamp(pos + d, 0, end - MIN_SIZE));
    return [start, end - start];
  }

  // The active preset follows every edit, so it never needs saving by hand.
  $effect(() => {
    const style = $state.snapshot(s.overlay);
    const preset = s.presets.find((p) => p.name === s.activePreset);
    if (preset) preset.style = style;
  });

  /** Picks `preset`; picking the active one again detaches the overlay from it. */
  function pickPreset(preset: Preset) {
    if (s.activePreset === preset.name) {
      s.activePreset = null;
      return;
    }
    s.overlay = $state.snapshot(preset.style);
    s.activePreset = preset.name;
  }

  function deletePreset(i: number) {
    if (s.presets[i].name === s.activePreset) s.activePreset = null;
    s.presets.splice(i, 1);
  }

  function savePreset() {
    const name = presetName.trim();
    if (!name) return;
    const style = $state.snapshot(s.overlay);
    const existing = s.presets.find((p) => p.name === name);
    if (existing) existing.style = style;
    else s.presets.push({ name, style });
    s.activePreset = name;
    presetName = "";
  }

  let version = $state("");
  getVersion().then((v) => (version = v));
  let checking = $state(false);
  let updateNote = $state<ReturnType<typeof explain> | null>(null);

  async function checkUpdate() {
    checking = true;
    updateNote = null;
    try {
      const u = await api.updateCheck(true);
      if (u) showUpdate(u);
      else updateNote = { text: "Установлена последняя версия.", detail: null };
    } catch (e) {
      updateNote = explain(e);
    } finally {
      checking = false;
    }
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
      <div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="stage"
          class:dragging={drag}
          style:aspect-ratio={aspect}
          style:width="min(100%, calc(60vh * {aspect}))"
          bind:clientWidth={stageWidth}
          onpointermove={onDrag}
          onpointerup={() => (drag = null)}
          onpointercancel={() => (drag = null)}
        >
          {#if frame}
            <img
              class="frame"
              src={frame}
              alt=""
              onload={(e) => {
                const img = e.currentTarget as HTMLImageElement;
                if (img.naturalHeight) aspect = img.naturalWidth / img.naturalHeight;
              }}
            />
          {/if}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="panel"
            style:left="{(box.x / W) * 100}%"
            style:top="{(box.y / H) * 100}%"
            style:width="{(box.w / W) * 100}%"
            style:height="{(box.h / H) * 100}%"
            onpointerdown={(e) => startDrag(e, "move")}
          >
            {#if panel}
              <img src={panel} alt="Превью чата" draggable="false" />
            {/if}
            {#each corners as [, right, bottom] (`${right}${bottom}`)}
              <span
                class="handle"
                style:left={right ? "100%" : "0"}
                style:top={bottom ? "100%" : "0"}
                style:cursor={right === bottom ? "nwse-resize" : "nesw-resize"}
                onpointerdown={(e) => startDrag(e, [right, bottom])}
              ></span>
            {/each}
          </div>
          {#if frameLoading && !frame}
            <span class="badge dim">Загружаем кадр…</span>
          {/if}
        </div>
        <p class="dim hint">
          Тяните панель, чтобы сдвинуть, за углы — чтобы изменить размер.
          {app.vod ? "Кадр из открытой записи, сообщения для примера." : "Откройте запись, чтобы увидеть её кадр."}
        </p>
        {#if frameError || panelError}
          <p class="error hint">{frameError ?? panelError}</p>
        {/if}
      </div>

      <div class="controls">
        <div class="control">
          <span>Прижать к углу</span>
          <div class="corners">
            {#each corners as [arrow, right, bottom] (arrow)}
              <button class="btn small" onclick={() => toCorner(right, bottom)}>{arrow}</button>
            {/each}
          </div>
        </div>

        <div class="pair">
          <label class="control">
            <span>X</span>
            <input class="field" type="number" min="0" max={W} bind:value={s.overlay.x} />
          </label>
          <label class="control">
            <span>Y</span>
            <input class="field" type="number" min="0" max={H} bind:value={s.overlay.y} />
          </label>
        </div>
        <label class="control">
          <span>Ширина <b>{s.overlay.width}</b></span>
          <input type="range" min={MIN_SIZE} max={W} step="10" bind:value={s.overlay.width} />
        </label>
        <label class="control">
          <span>Высота <b>{s.overlay.height}</b></span>
          <input type="range" min={MIN_SIZE} max={H} step="10" bind:value={s.overlay.height} />
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
            Подложка сообщений
            <b>{s.overlay.messageBackground > 0 ? `${Math.round(s.overlay.messageBackground * 100)}%` : "нет"}</b>
          </span>
          <input type="range" min="0" max="1" step="0.05" bind:value={s.overlay.messageBackground} />
        </label>
        <label class="control">
          <span>
            Время жизни сообщения
            <b>{s.overlay.lifetime > 0 ? `${s.overlay.lifetime} с` : "не исчезают"}</b>
          </span>
          <input type="range" min="0" max="120" step="1" bind:value={s.overlay.lifetime} />
        </label>
        <p class="dim hint">Пиксели для 1080p, на другом разрешении масштабируются.</p>
      </div>
    </div>
  </section>

  <section class="glass card">
    <h2>Пресеты</h2>
    <div class="presets">
      {#each s.presets as preset, i (preset.name)}
        <div class="preset">
          <button
            class="btn small"
            class:selected={s.activePreset === preset.name}
            title={s.activePreset === preset.name ? "Нажмите ещё раз, чтобы отвязать" : "Применить"}
            onclick={() => pickPreset(preset)}
          >
            {preset.name}
          </button>
          <button class="btn small" title="Удалить" onclick={() => deletePreset(i)}>✕</button>
        </div>
      {:else}
        <span class="dim">Пока нет. Сохраните текущий оверлей под именем.</span>
      {/each}
    </div>
    <form
      class="line"
      onsubmit={(e) => {
        e.preventDefault();
        savePreset();
      }}
    >
      <input class="field" placeholder="Имя нового пресета" bind:value={presetName} />
      <button class="btn primary" disabled={!presetName.trim()}>Создать</button>
    </form>
    <p class="dim hint">
      Всё сохраняется само. Правки оверлея попадают в выбранный пресет, и нарезка сразу идёт с ним.
    </p>
  </section>

  <section class="glass card">
    <h2>Фильтр чата</h2>
    <div class="general">
      <span>Боты</span>
      <div>
        <label class="check">
          <input type="checkbox" bind:checked={s.chatFilter.hideBots} />
          Скрывать сообщения ботов
        </label>
        <textarea
          class="field bots"
          rows="3"
          disabled={!s.chatFilter.hideBots}
          value={s.chatFilter.bots.join(", ")}
          onchange={(e) =>
            (s.chatFilter.bots = e.currentTarget.value
              .split(/[,\n]/)
              .map((b) => b.trim())
              .filter(Boolean))}
        ></textarea>
        <p class="dim hint">Ники через запятую, регистр не важен.</p>
      </div>
      <span>Команды</span>
      <label class="check">
        <input type="checkbox" bind:checked={s.chatFilter.hideCommands} />
        Скрывать команды (сообщения, начинающиеся с «!»)
      </label>
    </div>
  </section>

  <SubtitleSettings />

  <section class="glass card">
    <h2>Видео</h2>
    <div class="general">
      <span>Кодировщик</span>
      <div>
        <select class="field" bind:value={s.output.encoder}>
          <option value="auto">Авто{gpus ? ` · ${gpus.length ? ENCODERS[gpus[0]] : "процессор"}` : ""}</option>
          <option value="cpu">Процессор (x264 / x265)</option>
          {#each GPUS as e (e)}
            <option value={e} disabled={!!gpus && !gpus.includes(e)}>
              {ENCODERS[e]}{gpus && !gpus.includes(e) ? " · нет" : ""}
            </option>
          {/each}
        </select>
        <p class="dim hint">
          Видеокарта кодирует в несколько раз быстрее процессора. Если она даст сбой, клип перекодируется на процессоре.
        </p>
      </div>
      <span>Кодек</span>
      <div class="line">
        <button class="btn" class:selected={s.output.codec === "h264"} onclick={() => (s.output.codec = "h264")}>
          H.264
        </button>
        <button class="btn" class:selected={s.output.codec === "hevc"} onclick={() => (s.output.codec = "hevc")}>
          H.265
        </button>
        <span class="dim hint inline">H.265 меньше по размеру, H.264 открывается везде.</span>
      </div>
      <span>Качество</span>
      <div class="line">
        {#each QUALITIES as [q, label] (q)}
          <button class="btn" class:selected={s.output.quality === q} onclick={() => (s.output.quality = q)}>
            {label}
          </button>
        {/each}
      </div>
      <span>Чат</span>
      <div>
        <div class="line">
          {#each CHAT_OUTPUTS as [o, label] (o)}
            <button class="btn" class:selected={s.chatOutput === o} onclick={() => (s.chatOutput = o)}>{label}</button>
          {/each}
        </div>
        <p class="dim hint">
          Отдельный файл — видео одного чата с прозрачным фоном (ProRes 4444, <code>.chat.mov</code> рядом с клипом),
          чтобы двигать его при монтаже.
        </p>
      </div>
    </div>
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
          записи (у файла с диска — его имя), {"{start}"} и {"{end}"} — время, {"{n}"} — номер клипа.
        </p>
      </div>
      <span>Токен Twitch</span>
      <div>
        <div class="line">
          <input
            class="field"
            type={showToken ? "text" : "password"}
            autocomplete="off"
            spellcheck="false"
            placeholder="Не задан"
            value={s.twitchToken ?? ""}
            oninput={(e) => (s.twitchToken = e.currentTarget.value.trim() || null)}
          />
          <button class="btn" onclick={() => (showToken = !showToken)}>{showToken ? "Скрыть" : "Показать"}</button>
        </div>
        <p class="dim hint">
          Нужен только для записей «для подписчиков», аккаунт должен иметь к ним доступ. Это значение cookie
          <code>auth-token</code>: в браузере, где вы вошли в Twitch, откройте twitch.tv, нажмите F12 → «Хранилище»
          (Firefox) или Application (Chrome) → Cookies → twitch.tv. Токен даёт доступ к аккаунту: он хранится только на
          этом компьютере, никому его не передавайте. После выхода из аккаунта в браузере он перестаёт работать.
        </p>
      </div>
      <span>Обновления</span>
      <div>
        <button class="btn" disabled={checking} onclick={checkUpdate}>
          {checking ? "Проверяем…" : "Проверить обновления"}
        </button>
        <p class="dim hint" title={updateNote?.detail}>
          WClip {version}{updateNote ? ` · ${updateNote.text}` : ""}
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
    gap: 20px;
  }

  .stage {
    position: relative;
    margin: 0 auto;
    overflow: hidden;
    border-radius: 12px;
    border: 1px solid var(--outline);
    background: linear-gradient(135deg, var(--surface-highest), var(--bg));
    touch-action: none;
    user-select: none;
  }

  .frame {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .panel {
    position: absolute;
    outline: 1px dashed var(--primary);
    cursor: move;
  }

  .panel img {
    display: block;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }

  .handle {
    position: absolute;
    width: 12px;
    height: 12px;
    margin: -6px 0 0 -6px;
    border-radius: 3px;
    background: var(--primary);
    opacity: 0;
    transition: opacity 0.15s;
  }

  .stage:hover .handle,
  .stage.dragging .handle {
    opacity: 1;
  }

  .badge {
    position: absolute;
    left: 12px;
    top: 10px;
    font-size: 13px;
  }

  .controls {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 16px 28px;
    align-items: start;
  }

  .controls > .hint {
    grid-column: 1 / -1;
    margin: 0;
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

  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
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
    align-items: center;
  }

  .hint.inline {
    margin: 0;
  }

  .check {
    display: flex;
    gap: 8px;
    align-items: center;
    padding-top: 9px;
    cursor: pointer;
  }

  .bots {
    width: 100%;
    box-sizing: border-box;
    margin-top: 10px;
    resize: vertical;
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
