// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { explain } from "./errors.ts";

test("explain words known kinds and keeps the raw text", () => {
  const e = explain({ kind: "network", detail: "ffmpeg: Connection timed out" });
  assert.match(e.text, /Нет связи/);
  assert.equal(e.detail, "ffmpeg: Connection timed out");
});

test("explain passes other errors through", () => {
  assert.deepEqual(explain({ kind: "other", detail: "boom" }), { text: "boom", detail: null });
  assert.deepEqual(explain("plain"), { text: "plain", detail: null });
});

test("explain tells a stale token from a missing one", () => {
  assert.match(explain({ kind: "bad-token", detail: "HTTP Error 401" }).text, /не принял токен/);
  assert.match(explain({ kind: "sub-only", detail: "" }).text, /токен Twitch в настройках/);
});
