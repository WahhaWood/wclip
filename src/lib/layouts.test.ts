// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { CANVAS_H, PRESETS, SOURCE_ASPECT, moveArea, pieceHeight, resizeArea } from "./layouts.ts";

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
  const grown = resizeArea(a, 0.5, 0.05, true, true);
  assert.equal(grown.x + grown.w, 1);
  assert.equal(grown.h.toFixed(2), "0.45");
  const shrunk = resizeArea(a, 0.9, 0, false, false);
  assert.ok(shrunk.w > 0.02 && Math.abs(shrunk.x + shrunk.w - 0.9) < 1e-9);
});
