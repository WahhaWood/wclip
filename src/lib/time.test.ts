// Run with `npm test` (node's built-in runner; node strips the types itself).
import { test } from "node:test";
import assert from "node:assert/strict";
import { formatClock, formatTime, parseClock, parseIntervalList, parseTime } from "./time.ts";

test("parseTime accepts clock and unit forms", () => {
  assert.equal(parseTime("1:02:03"), 3723);
  assert.equal(parseTime("02:03"), 123);
  assert.equal(parseTime("45"), 45);
  assert.equal(parseTime(" 0:00:10.5 "), 10.5);
  assert.equal(parseTime("1:30,25"), 90.25);
  assert.equal(parseTime("1h2m3s"), 3723);
  assert.equal(parseTime("2m"), 120);
  assert.equal(parseTime("90s"), 90);
  assert.equal(parseTime("75m"), 4500);
});

test("parseTime rejects junk", () => {
  for (const bad of ["", "abc", "1:60", "1:02:60", "1:2:3:4", "-5", "1h75m", "1::2"]) {
    assert.equal(parseTime(bad), null, bad);
  }
});

test("formatTime", () => {
  assert.equal(formatTime(3723), "1:02:03");
  assert.equal(formatTime(65), "1:05");
  assert.equal(formatTime(5), "0:05");
  assert.equal(formatTime(10.5), "0:10.5");
  assert.equal(parseTime(formatTime(4000.25)), 4000.25);
});

test("parseIntervalList", () => {
  const { intervals, bad } = parseIntervalList(
    [
      "1:00:00 1:00:30",
      "1:05:00 - 1:06:10 Funny moment",
      "",
      "10:00–10:45: boss fight",
      "1h2m3s 1h3m",
      "just a note",
      "2:00 to 2:30",
    ].join("\n"),
  );
  assert.deepEqual(intervals, [
    { start: 3600, end: 3630, name: "" },
    { start: 3900, end: 3970, name: "Funny moment" },
    { start: 600, end: 645, name: "boss fight" },
    { start: 3723, end: 3780, name: "" },
    { start: 120, end: 150, name: "" },
  ]);
  assert.deepEqual(bad, ["just a note"]);
});

test("clock times round-trip and take the date from near", () => {
  // 2026-10-19 20:00:03 as naive seconds (days_from_civil on the Rust side).
  const t = 20745 * 86400 + 72003;
  assert.equal(formatClock(t), "2026-10-19 20:00:03");
  assert.equal(parseClock("2026-10-19 20:00:03", 0), t);
  assert.equal(parseClock(" 21:30:00 ", t), 20745 * 86400 + 77400);
  for (const bad of ["", "20:00", "25:00:00", "2026-02-30 10:00:00", "2026-10-19"]) {
    assert.equal(parseClock(bad, t), null, bad);
  }
});
