// 123 W2 — TS twins of `crates/surface/src/render/charts.rs`: the one-line header every chart lowering opens with
// (the svg `<desc>`, so the a11y text and the terminal agree) and the compact number format.

/** `BarChart: CI runs per day — 2 series` / `Gantt: 6 tasks` — `describe()` in `charts.rs`. */
export function describe(name: string, title: string | null | undefined, n: number, one: string, many = `${one}s`): string {
  const count = `${n} ${n === 1 ? one : many}`
  return title ? `${name}: ${title} — ${count}` : `${name}: ${count}`
}

/** Integers plain, otherwise up to two decimals with trailing zeros trimmed — `num()` in `charts.rs`. */
export function num(v: number): string {
  if (Math.abs(v - Math.round(v)) < 1e-9) return String(Math.round(v))
  return v.toFixed(2).replace(/0+$/, '').replace(/\.$/, '')
}

/** `42 ms` / `42`. */
export function withUnit(v: number, unit?: string | null): string {
  return unit ? `${num(v)} ${unit}` : num(v)
}

/** `+12` / `-5`. */
export function signed(v: number): string {
  return v < 0 ? `-${num(-v)}` : `+${num(v)}`
}

/** `Oct 8` / `Oct 8, 09:00` for tooltips and the Gantt / Timeline rows (UTC, as the Rust side reads them). */
export function shortDate(d: Date, withTime = false): string {
  const mon = d.toLocaleString('en', { month: 'short', timeZone: 'UTC' })
  const day = `${mon} ${d.getUTCDate()}`
  return withTime ? `${day}, ${String(d.getUTCHours()).padStart(2, '0')}:${String(d.getUTCMinutes()).padStart(2, '0')}` : day
}
