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

const pad = (n: number) => String(n).padStart(2, "0");

/** `YYYY-MM-DD HH:MM:SS` of naive local seconds (see `chat::log` on the Rust side). */
export function formatClock(seconds: number): string {
  const d = new Date(Math.round(seconds) * 1000);
  const date = `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
  return `${date} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
}

/**
 * Naive local seconds from `YYYY-MM-DD HH:MM:SS`, or from `HH:MM:SS` on the date of
 * `near` (also naive seconds); `null` when the text is not a date and time.
 */
export function parseClock(text: string, near: number): number | null {
  const m = /^\s*(?:(\d{4})-(\d{2})-(\d{2})[ T]+)?(\d{1,2}):(\d{2}):(\d{2})\s*$/.exec(text);
  if (!m) return null;
  const [h, min, s] = [m[4], m[5], m[6]].map(Number);
  if (h > 23 || min > 59 || s > 59) return null;
  const day = m[1]
    ? Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])) / 1000
    : Math.floor(near / 86400) * 86400;
  if (m[1] && formatClock(day).slice(0, 10) !== `${m[1]}-${m[2]}-${m[3]}`) return null;
  return day + h * 3600 + min * 60 + s;
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
