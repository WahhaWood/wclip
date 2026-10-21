<script lang="ts">
  import type { Settings } from "$lib/api";
  import { app } from "$lib/state.svelte";

  /**
   * Subtitles on a preview of the clip, dragged anywhere on it. `x`, `y`: the
   * caption's middle as fractions of the parent; `k`: screen pixels per pixel of a
   * frame whose short side is 1080.
   */
  let { x = $bindable(), y = $bindable(), k }: { x: number; y: number; k: number } = $props();

  const style = $derived((app.settings as Settings).subtitleStyle);
  const hex = (c: [number, number, number]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");

  // The first edited caption of the intervals, else an example of the same length.
  const words = $derived.by(() => {
    const text = app.rows.flatMap((r) => r.captions ?? []).find((c) => c.text.trim())?.text;
    const list = text ? text.split(/\s+/).filter(Boolean) : ["Пример", "субтитров", "здесь"].slice(0, Math.max(1, style.words));
    return style.uppercase ? list.map((w) => w.toUpperCase()) : list;
  });
  const lit = $derived(Math.min(1, words.length - 1));

  /** libass keeps this far from the left and right edges (`MarginL`, `MarginR`). */
  const MARGIN = 60;
  /** Distance from the middle that snaps to it. */
  const SNAP = 0.02;
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, hi));
  const round = (v: number) => Math.round(v * 1000) / 1000;

  let drag: { px: number; py: number; x: number; y: number; w: number; h: number } | null = null;

  function down(e: PointerEvent) {
    const stage = (e.currentTarget as HTMLElement).parentElement?.getBoundingClientRect();
    if (e.button !== 0 || !stage?.width) return;
    e.preventDefault();
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    drag = { px: e.clientX, py: e.clientY, x, y, w: stage.width, h: stage.height };
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    const nx = clamp(drag.x + (e.clientX - drag.px) / drag.w, 0.05, 0.95);
    x = Math.abs(nx - 0.5) < SNAP ? 0.5 : round(nx);
    y = round(clamp(drag.y + (e.clientY - drag.py) / drag.h, 0.05, 0.95));
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="subs"
  title="Тяните, чтобы поставить субтитры"
  style:left="{x * 100}%"
  style:top="{y * 100}%"
  style:max-width="calc(100% - {2 * MARGIN * k}px)"
  style:font-size="{style.size * k}px"
  style:-webkit-text-stroke="{Math.max(1, 2 * style.outline * k)}px var(--bg)"
  style:color={hex(style.color)}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={() => (drag = null)}
  onpointercancel={() => (drag = null)}
>
  {#each words as word, i (i)}
    <span style:color={i === lit ? hex(style.highlight) : undefined}>{word}</span>{" "}
  {/each}
</div>

<style>
  .subs {
    position: absolute;
    width: max-content;
    transform: translate(-50%, -50%);
    text-align: center;
    font-family: Inter, sans-serif;
    font-weight: 700;
    line-height: 1.15;
    paint-order: stroke fill;
    outline: 1px dashed var(--primary);
    cursor: move;
    touch-action: none;
    user-select: none;
    z-index: 2;
  }
</style>
