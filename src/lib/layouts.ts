// Vertical clip layouts: built-in presets and the geometry the editor needs.
import type { Area, ChatBox, Layout, Piece } from "./api.ts";

export const CANVAS_W = 1080;
export const CANVAS_H = 1920;
/** Twitch VODs; the editor uses the real frame once it has loaded. */
export const SOURCE_ASPECT = 16 / 9;

/** Height of `piece` on the canvas for a source frame of `aspect` (width / height). */
export function pieceHeight(piece: Piece, aspect: number): number {
  return Math.round((piece.width * piece.src.h) / (piece.src.w * aspect));
}

/** The part of a 16:9 frame that fills the canvas at full height, `x` its left edge. */
function column(x: number): Area {
  return { x, y: 0, w: 9 / 16 / SOURCE_ASPECT, h: 1 };
}

const chat = (y: number, height: number): ChatBox => ({ x: 40, y, width: 1000, height, fontSize: 30 });

/** Built-in layouts; the camera is assumed in the top-right corner, a common spot. */
export const PRESETS: Layout[] = [
  {
    name: "Камера сверху, игра снизу",
    pieces: [
      { name: "Игра", src: { x: 0.15, y: 0, w: 0.7, h: 1 }, x: 0, y: 608, width: 1080 },
      { name: "Камера", src: { x: 0.74, y: 0, w: 0.26, h: 0.26 }, x: 0, y: 0, width: 1080 },
    ],
    blur: true,
    color: [0, 0, 0],
    chat: chat(1490, 400),
  },
  {
    name: "Кадр целиком, размытый фон",
    pieces: [{ name: "Кадр", src: { x: 0, y: 0, w: 1, h: 1 }, x: 0, y: 656, width: 1080 }],
    blur: true,
    color: [0, 0, 0],
    chat: chat(1300, 560),
  },
  {
    name: "Середина кадра",
    pieces: [{ name: "Кадр", src: column((1 - 9 / 16 / SOURCE_ASPECT) / 2), x: 0, y: 0, width: 1080 }],
    blur: true,
    color: [0, 0, 0],
    chat: chat(1220, 560),
  },
];

export function copyLayout(layout: Layout): Layout {
  return structuredClone(layout);
}

/** A new piece in the middle of the frame and of the canvas. */
export function newPiece(n: number): Piece {
  return { name: `Фрагмент ${n}`, src: { x: 0.35, y: 0.3, w: 0.3, h: 0.4 }, x: 240, y: 660, width: 600 };
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, hi));

/** Smallest area side, as a fraction of the frame. */
export const MIN_AREA = 0.03;

/** `area` moved by (dx, dy) fractions, kept inside the frame. */
export function moveArea(area: Area, dx: number, dy: number): Area {
  return { ...area, x: clamp(area.x + dx, 0, 1 - area.w), y: clamp(area.y + dy, 0, 1 - area.h) };
}

/**
 * `area` with a corner dragged by (dx, dy) fractions; `right` and `bottom` say
 * which corner. The opposite corner stays.
 */
export function resizeArea(area: Area, dx: number, dy: number, right: boolean, bottom: boolean): Area {
  const [x, w] = resizeAxis(area.x, area.w, dx, right);
  const [y, h] = resizeAxis(area.y, area.h, dy, bottom);
  return { x, y, w, h };
}

function resizeAxis(pos: number, size: number, d: number, far: boolean): [number, number] {
  if (far) return [pos, clamp(size + d, MIN_AREA, 1 - pos)];
  const end = pos + size;
  const start = clamp(pos + d, 0, end - MIN_AREA);
  return [start, end - start];
}
