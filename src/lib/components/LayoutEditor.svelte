<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Layout, type Settings } from "$lib/api";
  import { explain } from "$lib/errors";
  import {
    CANVAS_H,
    CANVAS_W,
    PRESETS,
    SOURCE_ASPECT,
    copyLayout,
    moveArea,
    newPiece,
    pieceHeight,
    resizeArea,
  } from "$lib/layouts";
  import { app } from "$lib/state.svelte";
  import { parseTime } from "$lib/time";

  // The parent renders this screen only once settings are loaded.
  const s = $derived(app.settings as Settings);
  const layout = $derived(s.layout);

  let frame = $state<string | null>(null);
  let frameError = $state<string | null>(null);
  let panel = $state<string | null>(null);
  let panelError = $state<string | null>(null);
  /** Width / height of the source frame. */
  let aspect = $state(SOURCE_ASPECT);
  /** Index of the selected piece. */
  let selected = $state(0);
  let saveName = $state("");
  let sourceWidth = $state(0);
  let canvasWidth = $state(0);
  /** Canvas pixels per screen pixel. */
  const canvasScale = $derived(canvasWidth ? CANVAS_W / canvasWidth : 0);

  const MIN_WIDTH = 100;
  const MIN_CHAT = 150;
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, hi));
  const pct = (v: number, of: number) => `${(v / of) * 100}%`;

  // The frame at the first interval start of the open video, else its middle.
  const previewTime = $derived.by(() => {
    if (!app.vod) return 0;
    const first = app.rows.map((r) => parseTime(r.start)).find((t) => t !== null);
    return first ?? Math.floor(app.vod.duration / 2);
  });

  function blobUrl(old: string | null, bytes: ArrayBuffer, type: string) {
    if (old) URL.revokeObjectURL(old);
    return URL.createObjectURL(new Blob([bytes], { type }));
  }

  onMount(() => () => {
    if (frame) URL.revokeObjectURL(frame);
    if (panel) URL.revokeObjectURL(panel);
  });

  $effect(() => {
    const playlist = app.vod?.playlist;
    const time = previewTime;
    if (!playlist) return;
    let stale = false;
    api.previewFrame(playlist, time).then(
      (jpeg) => {
        if (stale) return;
        frame = blobUrl(frame, jpeg, "image/jpeg");
        frameError = null;
      },
      (e) => {
        if (!stale) frameError = explain(e).text;
      },
    );
    return () => (stale = true);
  });

  // The chat panel in the overlay style, at the size of the chat box.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  $effect(() => {
    const c = layout.chat;
    const style = { ...$state.snapshot(s.overlay), x: 0, y: 0, width: c.width, height: c.height, fontSize: c.fontSize };
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const mine = ++seq;
      try {
        const png = await api.overlayPanel(style);
        if (mine !== seq) return;
        panel = blobUrl(panel, png, "image/png");
        panelError = null;
      } catch (e) {
        if (mine === seq) panelError = explain(e).text;
      }
    }, 60);
  });

  // A saved layout follows every edit, so it never needs saving by hand.
  $effect(() => {
    const current = $state.snapshot(layout);
    const i = s.layouts.findIndex((l) => l.name === current.name);
    if (i >= 0 && JSON.stringify(s.layouts[i]) !== JSON.stringify(current)) s.layouts[i] = current;
  });

  $effect(() => {
    if (selected >= layout.pieces.length) selected = Math.max(0, layout.pieces.length - 1);
  });

  type Target =
    | { kind: "src"; i: number; corner: [boolean, boolean] | null }
    | { kind: "piece"; i: number; resize: boolean }
    | { kind: "chat"; corner: [boolean, boolean] | null };
  type Drag = { target: Target; px: number; py: number; start: Layout };
  let drag = $state<Drag | null>(null);

  function startDrag(e: PointerEvent, target: Target) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    if (target.kind !== "chat") selected = target.i;
    drag = { target, px: e.clientX, py: e.clientY, start: $state.snapshot(layout) };
  }

  function onDrag(e: PointerEvent) {
    if (!drag) return;
    const { target, start } = drag;
    const mx = e.clientX - drag.px;
    const my = e.clientY - drag.py;
    if (target.kind === "src") {
      if (!sourceWidth) return;
      const dx = mx / sourceWidth;
      const dy = my / (sourceWidth / aspect);
      const area = start.pieces[target.i].src;
      layout.pieces[target.i].src = target.corner
        ? resizeArea(area, dx, dy, ...target.corner)
        : moveArea(area, dx, dy);
      return;
    }
    const dx = Math.round(mx * canvasScale);
    const dy = Math.round(my * canvasScale);
    if (target.kind === "piece") {
      const from = start.pieces[target.i];
      const piece = layout.pieces[target.i];
      if (target.resize) piece.width = clamp(from.width + dx, MIN_WIDTH, 2 * CANVAS_W);
      else [piece.x, piece.y] = [from.x + dx, from.y + dy];
      return;
    }
    const from = start.chat;
    const chat = layout.chat;
    if (!target.corner) {
      chat.x = clamp(from.x + dx, 0, CANVAS_W - from.width);
      chat.y = clamp(from.y + dy, 0, CANVAS_H - from.height);
      return;
    }
    const [right, bottom] = target.corner;
    [chat.x, chat.width] = resizeAxis(from.x, from.width, dx, right, CANVAS_W);
    [chat.y, chat.height] = resizeAxis(from.y, from.height, dy, bottom, CANVAS_H);
  }

  function resizeAxis(pos: number, size: number, d: number, far: boolean, limit: number): [number, number] {
    if (far) return [pos, clamp(size + d, MIN_CHAT, limit - pos)];
    const end = pos + size;
    const begin = clamp(pos + d, 0, end - MIN_CHAT);
    return [begin, end - begin];
  }

  const corners: [boolean, boolean][] = [
    [false, false],
    [true, false],
    [false, true],
    [true, true],
  ];

  function pick(l: Layout) {
    s.layout = copyLayout(l);
    selected = 0;
    if (s.layouts.some((x) => x.name === l.name) && app.vod && !app.vod.local) s.channelLayouts[app.vod.channel] = l.name;
  }

  function save() {
    const name = saveName.trim();
    if (!name) return;
    const copy = { ...copyLayout($state.snapshot(layout)), name };
    const i = s.layouts.findIndex((l) => l.name === name);
    if (i >= 0) s.layouts[i] = copy;
    else s.layouts.push(copy);
    s.layout = copyLayout(copy);
    if (app.vod && !app.vod.local) s.channelLayouts[app.vod.channel] = name;
    saveName = "";
  }

  function remove(i: number) {
    const name = s.layouts[i].name;
    s.layouts.splice(i, 1);
    for (const [channel, l] of Object.entries(s.channelLayouts)) if (l === name) delete s.channelLayouts[channel];
  }

  function addPiece() {
    layout.pieces.push(newPiece(layout.pieces.length + 1));
    selected = layout.pieces.length - 1;
  }

  /** Moves piece `i` one layer up (`d` 1) or down (-1). */
  function reorder(i: number, d: number) {
    const j = i + d;
    if (j < 0 || j >= layout.pieces.length) return;
    const pieces = layout.pieces;
    [pieces[i], pieces[j]] = [pieces[j], pieces[i]];
    selected = j;
  }

  const hex = (c: [number, number, number]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
  function setColor(value: string) {
    const n = parseInt(value.slice(1), 16);
    layout.color = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }

  const piece = $derived(layout.pieces[selected]);
  const savedName = $derived(s.layouts.some((l) => l.name === layout.name) ? layout.name : null);
</script>

<div class="screen">
  <section class="glass card">
    <h2>Вертикальное видео</h2>
    <div class="presets">
      {#each PRESETS as preset (preset.name)}
        <button class="btn small" onclick={() => pick(preset)}>{preset.name}</button>
      {/each}
    </div>
    <div class="presets">
      {#each s.layouts as saved, i (saved.name)}
        <div class="preset">
          <button class="btn small" class:selected={savedName === saved.name} onclick={() => pick(saved)}>{saved.name}</button>
          <button class="btn small" title="Удалить" onclick={() => remove(i)}>✕</button>
        </div>
      {:else}
        <span class="dim">Своих раскладок пока нет: подгоните одну из готовых и сохраните.</span>
      {/each}
    </div>
    <form
      class="line"
      onsubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <input class="field" placeholder={app.vod && !app.vod.local ? `Например, ${app.vod.channel}` : "Имя раскладки"} bind:value={saveName} />
      <button class="btn primary" disabled={!saveName.trim()}>Сохранить</button>
    </form>
    <p class="dim hint">
      {savedName ? `Правки сохраняются в «${savedName}».` : "Правки готовой раскладки не сохраняются, пока не дадите ей имя."}
      Сохранённая раскладка запоминается для канала и выбирается сама, когда откроете его запись.
    </p>
  </section>

  <section class="glass card">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="editor" onpointermove={onDrag} onpointerup={() => (drag = null)} onpointercancel={() => (drag = null)}>
      <div class="column">
        <h3>Исходный кадр</h3>
        <div class="stage source" class:dragging={drag} style:aspect-ratio={aspect} bind:clientWidth={sourceWidth}>
          {#if frame}
            <img
              class="fill"
              src={frame}
              alt=""
              draggable="false"
              onload={(e) => {
                const img = e.currentTarget as HTMLImageElement;
                if (img.naturalHeight) aspect = img.naturalWidth / img.naturalHeight;
              }}
            />
          {/if}
          {#each layout.pieces as p, i (i)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="area"
              class:active={i === selected}
              style:left={pct(p.src.x, 1)}
              style:top={pct(p.src.y, 1)}
              style:width={pct(p.src.w, 1)}
              style:height={pct(p.src.h, 1)}
              onpointerdown={(e) => startDrag(e, { kind: "src", i, corner: null })}
            >
              <span class="label">{p.name}</span>
              {#each corners as corner (corner.join())}
                <span
                  class="handle"
                  style:left={corner[0] ? "100%" : "0"}
                  style:top={corner[1] ? "100%" : "0"}
                  style:cursor={corner[0] === corner[1] ? "nwse-resize" : "nesw-resize"}
                  onpointerdown={(e) => startDrag(e, { kind: "src", i, corner })}
                ></span>
              {/each}
            </div>
          {/each}
        </div>
        <p class="dim hint">
          Рамки — какие части кадра попадут в ролик: камера, игра. {app.vod
            ? "Кадр из открытой записи."
            : "Откройте запись, чтобы увидеть её кадр."}
        </p>
        {#if frameError}
          <p class="error hint">{frameError}</p>
        {/if}

        {#if piece}
          <div class="controls">
            <label class="control">
              <span>Фрагмент</span>
              <input class="field" bind:value={piece.name} />
            </label>
            <div class="control">
              <span>Слой</span>
              <div class="line">
                <button class="btn small" disabled={selected === layout.pieces.length - 1} onclick={() => reorder(selected, 1)}>
                  Выше
                </button>
                <button class="btn small" disabled={selected === 0} onclick={() => reorder(selected, -1)}>Ниже</button>
                <button class="btn small" onclick={() => layout.pieces.splice(selected, 1)}>Удалить</button>
              </div>
            </div>
            <div class="control">
              <span>На холсте</span>
              <div class="line">
                <button class="btn small" onclick={() => (piece.x = Math.round((CANVAS_W - piece.width) / 2))}>По центру</button>
                <button class="btn small" onclick={() => ([piece.x, piece.width] = [0, CANVAS_W])}>Во всю ширину</button>
              </div>
            </div>
          </div>
        {/if}
        <div class="line pieces">
          {#each layout.pieces as p, i (i)}
            <button class="btn small" class:selected={i === selected} onclick={() => (selected = i)}>{p.name}</button>
          {/each}
          <button class="btn small" onclick={addPiece}>+ Фрагмент</button>
        </div>
      </div>

      <div class="column narrow">
        <h3>Ролик 9:16</h3>
        <div
          class="stage canvas"
          class:dragging={drag}
          bind:clientWidth={canvasWidth}
          style:background={layout.blur ? undefined : hex(layout.color)}
        >
          {#if frame && layout.blur}
            <img class="fill blur" src={frame} alt="" draggable="false" style:filter="blur({canvasWidth * 0.045}px)" />
          {/if}
          {#each layout.pieces as p, i (i)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="piece"
              class:active={i === selected}
              style:left={pct(p.x, CANVAS_W)}
              style:top={pct(p.y, CANVAS_H)}
              style:width={pct(p.width, CANVAS_W)}
              style:height={pct(pieceHeight(p, aspect), CANVAS_H)}
              onpointerdown={(e) => startDrag(e, { kind: "piece", i, resize: false })}
            >
              <div class="crop">
                {#if frame}
                  <img
                    src={frame}
                    alt=""
                    draggable="false"
                    style:width={pct(1, p.src.w)}
                    style:height={pct(1, p.src.h)}
                    style:left={pct(-p.src.x, p.src.w)}
                    style:top={pct(-p.src.y, p.src.h)}
                  />
                {/if}
              </div>
              <span
                class="handle"
                style:left="100%"
                style:top="100%"
                style:cursor="nwse-resize"
                onpointerdown={(e) => startDrag(e, { kind: "piece", i, resize: true })}
              ></span>
            </div>
          {/each}
          {#if s.chat}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="chat"
              style:left={pct(layout.chat.x, CANVAS_W)}
              style:top={pct(layout.chat.y, CANVAS_H)}
              style:width={pct(layout.chat.width, CANVAS_W)}
              style:height={pct(layout.chat.height, CANVAS_H)}
              onpointerdown={(e) => startDrag(e, { kind: "chat", corner: null })}
            >
              {#if panel}
                <img src={panel} alt="Превью чата" draggable="false" />
              {/if}
              {#each corners as corner (corner.join())}
                <span
                  class="handle"
                  style:left={corner[0] ? "100%" : "0"}
                  style:top={corner[1] ? "100%" : "0"}
                  style:cursor={corner[0] === corner[1] ? "nwse-resize" : "nesw-resize"}
                  onpointerdown={(e) => startDrag(e, { kind: "chat", corner })}
                ></span>
              {/each}
            </div>
          {/if}
        </div>
        {#if panelError}
          <p class="error hint">{panelError}</p>
        {/if}
        <div class="controls">
          <div class="control">
            <span>Фон</span>
            <div class="line">
              <button class="btn small" class:selected={layout.blur} onclick={() => (layout.blur = true)}>Размытый кадр</button>
              <button class="btn small" class:selected={!layout.blur} onclick={() => (layout.blur = false)}>Цвет</button>
              {#if !layout.blur}
                <input type="color" value={hex(layout.color)} oninput={(e) => setColor(e.currentTarget.value)} />
              {/if}
            </div>
          </div>
          {#if s.chat}
            <label class="control">
              <span>Размер текста чата <b>{layout.chat.fontSize}</b></span>
              <input type="range" min="16" max="64" step="1" bind:value={layout.chat.fontSize} />
            </label>
          {/if}
        </div>
        <p class="dim hint">
          Тяните фрагменты и чат, за угол — размер. Шрифт, фон и подложка чата — из настроек оверлея.
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

  .editor {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 28px;
    align-items: start;
  }

  .column {
    display: grid;
    gap: 12px;
    min-width: 0;
  }

  .narrow {
    width: min(340px, 32vh);
  }

  h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .stage {
    position: relative;
    overflow: hidden;
    border-radius: 12px;
    border: 1px solid var(--outline);
    background: linear-gradient(135deg, var(--surface-highest), var(--bg));
    touch-action: none;
    user-select: none;
  }

  .canvas {
    aspect-ratio: 9 / 16;
  }

  .fill {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .blur {
    transform: scale(1.1);
  }

  .area,
  .piece,
  .chat {
    position: absolute;
    cursor: move;
    box-sizing: border-box;
  }

  .area {
    outline: 1px dashed var(--text-dim);
    background: color-mix(in srgb, var(--bg) 20%, transparent);
  }

  .area.active {
    outline: 2px solid var(--primary);
    background: none;
  }

  .label {
    position: absolute;
    left: 6px;
    top: 4px;
    font-size: 12px;
    color: var(--text);
    text-shadow: 0 0 3px var(--bg);
    pointer-events: none;
  }

  .crop {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }

  .crop img {
    position: absolute;
    max-width: none;
    pointer-events: none;
  }

  .piece.active {
    outline: 1px dashed var(--primary);
  }

  .chat {
    outline: 1px dashed var(--tertiary);
  }

  .chat img {
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
    z-index: 1;
  }

  .stage:hover .handle,
  .stage.dragging .handle {
    opacity: 1;
  }

  .controls {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 14px 24px;
    align-items: start;
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

  .hint {
    margin: 0;
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
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }

  form.line {
    flex-wrap: nowrap;
  }

  form .field {
    flex: 1;
  }

  form + .hint {
    margin-top: 8px;
  }

  @media (max-width: 860px) {
    .editor {
      grid-template-columns: 1fr;
    }

    .narrow {
      width: min(340px, 100%);
      margin: 0 auto;
    }
  }
</style>
