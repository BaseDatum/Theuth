// Design 123 §5.1 / §5.3 — the kernel's shared shapes. FROZEN for wave 123 (see README.md).

/** What every diagram host (`<NameView>`) takes: the wire props, and the apps' reactive `highlight` / `onSelect` (65 §3.5). */
export type DiagramHostProps<P> = {
  props: P
  /** Unit ids drawn `active` (a unit's own `danger`/`done` wins). */
  highlight?: readonly string[]
  /** A unit was pressed; the unit id is the apps' `$value`. */
  onSelect?: (id: string) => void
  /** The fence is still streaming: drop dangling references, keep the last good layout. */
  loading?: boolean
}

/** ts-rs spells `Option<T>` as a required `T | null`; on the wire the key is simply absent. */
export type Loose<T> = { [K in keyof T as null extends T[K] ? never : K]: T[K] } & { [K in keyof T as null extends T[K] ? K : never]?: T[K] | undefined }

export interface Size {
  w: number
  h: number
}
export interface Point {
  x: number
  y: number
}

/** The `GraphState` vocabulary every unit paints with (`spec.rs`). */
export type UnitState = 'default' | 'active' | 'done' | 'muted' | 'danger'
