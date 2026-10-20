export type IntervalDraft = { start: number; end: number; name: string };

const CLOCK = String.raw`\d+(?::\d{1,2}){0,2}(?:[.,]\d+)?`;
const UNITS = String.raw`(?:\d+h)?(?:\d+m)?(?:\d+(?:[.,]\d+)?s)?`;
const TIME = `(?:${CLOCK}|${UNITS})`;
const LINE = new RegExp(String.raw`^\s*(${TIME})\s*(?:[-–—]|\s|to|до)\s*(${TIME})(?:\s*[-–—:|]?\s+(.*))?\s*$`, "i");

/**
 * Seconds from `H:MM:SS`, `M:SS` or `S` (fraction allowed in the last part, comma or
 * dot) or from `1h2m3s` style; `null` when the text is not a time.
 */
export function parseTime(text: string): number | null {
  const t = text.trim().replace(",", ".");
  if (!t) return null;
  const units = /^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+(?:\.\d+)?)s)?$/i.exec(t);
  if (units && /[hms]/i.test(t)) {
    const [h, m, s] = [units[1], units[2], units[3]].map((v) => Number(v ?? 0));
    return m >= 60 && units[1] !== undefined ? null : h * 3600 + m * 60 + s;
  }
  if (!/^\d+(?::\d{1,2}){0,2}(?:\.\d+)?$/.test(t)) return null;
  const parts = t.split(":").map(Number);
  // Every part after the first is minutes or seconds.
  if (parts.slice(1).some((p) => p >= 60)) return null;
  return parts.reduce((acc, p) => acc * 60 + p, 0);
}

/** `H:MM:SS`, or `M:SS` under an hour; whole seconds unless there is a fraction. */
export function formatTime(seconds: number): string {
  const whole = Math.floor(seconds);
  const frac = seconds - whole;
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const ss = String(s).padStart(2, "0") + (frac > 0.0005 ? frac.toFixed(3).slice(1).replace(/0+$/, "") : "");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/**
 * One interval per line: `start end`, `start - end` or `start–end`, then an optional
 * clip name. Empty lines are skipped; lines that do not parse go to `bad`.
 */
export function parseIntervalList(text: string): { intervals: IntervalDraft[]; bad: string[] } {
  const intervals: IntervalDraft[] = [];
  const bad: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const m = LINE.exec(line);
    const start = m ? parseTime(m[1]) : null;
    const end = m ? parseTime(m[2]) : null;
    if (start === null || end === null) {
      bad.push(line.trim());
      continue;
    }
    intervals.push({ start, end, name: (m?.[3] ?? "").trim() });
  }
  return { intervals, bad };
}
