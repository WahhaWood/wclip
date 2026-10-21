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
    subtitlesX: 0.5,
    // On the seam between the camera and the game.
    subtitlesY: 0.32,
  },
  {
    name: "Кадр целиком, размытый фон",
    pieces: [{ name: "Кадр", src: { x: 0, y: 0, w: 1, h: 1 }, x: 0, y: 656, width: 1080 }],
    blur: true,
    color: [0, 0, 0],
    chat: chat(1300, 560),
    subtitlesX: 0.5,
    subtitlesY: 0.25,
  },
  {
    name: "Середина кадра",
    pieces: [{ name: "Кадр", src: column((1 - 9 / 16 / SOURCE_ASPECT) / 2), x: 0, y: 0, width: 1080 }],
    blur: true,
    color: [0, 0, 0],
    chat: chat(1220, 560),
    subtitlesX: 0.5,
    subtitlesY: 0.55,
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

/** A handle: the side it moves on each axis, -1 left or top, 1 right or bottom, 0 neither. */
export type Handle = [number, number];

/** Corners, then the middles of the sides. */
export const HANDLES: Handle[] = [
  [-1, -1],
  [1, -1],
  [-1, 1],
  [1, 1],
  [0, -1],
  [0, 1],
  [-1, 0],
  [1, 0],
];

export const handleCursor = ([hx, hy]: Handle) =>
  !hx ? "ns-resize" : !hy ? "ew-resize" : hx === hy ? "nwse-resize" : "nesw-resize";

/** `area` with handle `h` dragged by (dx, dy) fractions; the opposite sides stay. */
export function resizeArea(area: Area, dx: number, dy: number, [hx, hy]: Handle): Area {
  const [x, w] = hx ? resizeAxis(area.x, area.w, dx, hx > 0) : [area.x, area.w];
  const [y, h] = hy ? resizeAxis(area.y, area.h, dy, hy > 0) : [area.y, area.h];
  return { x, y, w, h };
}

function resizeAxis(pos: number, size: number, d: number, far: boolean): [number, number] {
  if (far) return [pos, clamp(size + d, MIN_AREA, 1 - pos)];
  const end = pos + size;
  const start = clamp(pos + d, 0, end - MIN_AREA);
  return [start, end - start];
}

/** Narrowest piece on the canvas, pixels. */
export const MIN_PIECE = 100;

/**
 * `piece` scaled by corner handle `h` dragged by (dx, dy) canvas pixels: the
 * proportions and the opposite corner stay.
 */
export function scalePiece(piece: Piece, dx: number, dy: number, [hx, hy]: Handle, aspect: number): Piece {
  const height = pieceHeight(piece, aspect);
  const byX = hx * dx;
  const byY = (hy * dy * piece.width) / height;
  const grow = Math.abs(byX) >= Math.abs(byY) ? byX : byY;
  const width = Math.round(clamp(piece.width + grow, MIN_PIECE, 2 * CANVAS_W));
  const next = { ...piece, width };
  if (hx < 0) next.x = piece.x + piece.width - width;
  if (hy < 0) next.y = piece.y + height - pieceHeight(next, aspect);
  return next;
}

/**
 * `piece` with side handle `h` dragged by (dx, dy) canvas pixels: the side shows
 * more or less of the frame at the same scale, so only that side moves.
 */
export function cropPiece(piece: Piece, dx: number, dy: number, h: Handle, aspect: number): Piece {
  // Canvas pixels per frame width and per frame height.
  const kx = piece.width / piece.src.w;
  const ky = kx / aspect;
  const src = resizeArea(piece.src, dx / kx, dy / ky, h);
  return {
    ...piece,
    src,
    x: piece.x + Math.round((src.x - piece.src.x) * kx),
    y: piece.y + Math.round((src.y - piece.src.y) * ky),
    width: Math.round(src.w * kx),
  };
}
