// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { CANVAS_H, PRESETS, SOURCE_ASPECT, cropPiece, moveArea, pieceHeight, resizeArea, scalePiece } from "./layouts.ts";

test("piece height keeps the proportions of its part of the frame", () => {
  const [middle] = PRESETS.find((p) => p.name === "Середина кадра")!.pieces;
  assert.equal(pieceHeight(middle, SOURCE_ASPECT), CANVAS_H);
  const whole = { name: "", src: { x: 0, y: 0, w: 1, h: 1 }, x: 0, y: 0, width: 1080 };
  assert.equal(pieceHeight(whole, SOURCE_ASPECT), 608);
  assert.equal(pieceHeight(whole, 4 / 3), 810);
});

test("presets stack without gaps or overlaps", () => {
  const preset = PRESETS[0];
  const [game, cam] = preset.pieces;
  assert.equal(cam.y + pieceHeight(cam, SOURCE_ASPECT), game.y);
  assert.ok(game.y + pieceHeight(game, SOURCE_ASPECT) <= preset.chat.y);
  assert.ok(preset.chat.y + preset.chat.height <= CANVAS_H);
});

test("areas stay inside the frame", () => {
  const a = { x: 0.5, y: 0.5, w: 0.4, h: 0.4 };
  assert.deepEqual(moveArea(a, 0.5, -1), { x: 0.6, y: 0, w: 0.4, h: 0.4 });
  const grown = resizeArea(a, 0.5, 0.05, [1, 1]);
  assert.equal(grown.x + grown.w, 1);
  assert.equal(grown.h.toFixed(2), "0.45");
  const shrunk = resizeArea(a, 0.9, 0, [-1, -1]);
  assert.ok(shrunk.w > 0.02 && Math.abs(shrunk.x + shrunk.w - 0.9) < 1e-9);
});

test("a side handle crops one side of a piece at the same scale", () => {
  const whole = { name: "", src: { x: 0, y: 0, w: 1, h: 1 }, x: 0, y: 600, width: 1080 };
  const right = cropPiece(whole, -540, 0, [1, 0], SOURCE_ASPECT);
  assert.deepEqual([right.x, right.y, right.width, pieceHeight(right, SOURCE_ASPECT)], [0, 600, 540, 608]);
  assert.equal(right.src.w, 0.5);
  const top = cropPiece(whole, 0, 304, [0, -1], SOURCE_ASPECT);
  assert.deepEqual([top.x, top.y, top.width, pieceHeight(top, SOURCE_ASPECT)], [0, 904, 1080, 304]);
  // The frame ends there: a side past it stops at the edge.
  assert.deepEqual(cropPiece(whole, 200, 0, [1, 0], SOURCE_ASPECT), whole);
  // Back out again from a cropped piece.
  const left = cropPiece(right, 0, 0, [-1, 0], SOURCE_ASPECT);
  assert.deepEqual(left, right);
  const wider = cropPiece(right, 270, 0, [1, 0], SOURCE_ASPECT);
  assert.equal(wider.width, 810);
});

test("a corner handle scales a piece and keeps the opposite corner", () => {
  const whole = { name: "", src: { x: 0, y: 0, w: 1, h: 1 }, x: 100, y: 600, width: 1080 };
  const small = scalePiece(whole, 540, 0, [-1, -1], SOURCE_ASPECT);
  assert.equal(small.width, 540);
  assert.equal(small.x + small.width, whole.x + whole.width);
  assert.equal(small.y + pieceHeight(small, SOURCE_ASPECT), whole.y + pieceHeight(whole, SOURCE_ASPECT));
  assert.deepEqual(small.src, whole.src);
  const big = scalePiece(whole, 0, 608, [1, 1], SOURCE_ASPECT);
  assert.equal(big.width, 2160);
  assert.deepEqual([big.x, big.y], [whole.x, whole.y]);
});
