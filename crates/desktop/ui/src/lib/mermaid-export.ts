// Design 30 §8.3: a rendered mermaid diagram as an image. Mermaid's svg carries its own
// `<style>` block (and `htmlLabels: false` keeps labels as `<text>`), so it is self-contained
// once it has a pixel size and an opaque background; nothing from the page is inlined.

/** A `viewBox` (`min-x min-y width height`) as numbers; `null` when absent or malformed. */
export function viewBox(attr: string | null | undefined): { x: number; y: number; width: number; height: number } | null {
  const n = attr?.trim().split(/[\s,]+/).map(Number) ?? []
  const [x, y, width, height] = n
  return n.length === 4 && n.every(Number.isFinite) && width! > 0 && height! > 0 ? { x: x!, y: y!, width: width!, height: height! } : null
}

/**
 * `markup` as a standalone svg document: explicit `width`/`height` from the viewBox (mermaid emits `width="100%"`,
 * which an `<img>` decoder sizes to nothing), the inline `max-width` style dropped, `xmlns` present, and a
 * `background` rect behind everything (white for portability: dark-theme diagrams paste onto light pages).
 * The rect is the viewBox plus a unit on each side, so a non-zero origin or the rounding to whole pixels leaves no
 * transparent strip; the viewport clips the excess.
 */
export function standalone(markup: string, background = '#fff'): { svg: string; width: number; height: number } {
  const open = /<svg\b[^>]*>/.exec(markup)?.[0]
  if (!open) throw new Error('no <svg> root')
  const box = viewBox(/\bviewBox="([^"]*)"/.exec(open)?.[1])
  if (!box) throw new Error('svg has no viewBox')
  const width = Math.ceil(box.width)
  const height = Math.ceil(box.height)
  let tag = open.replace(/\s(width|height|style)="[^"]*"/g, '')
  if (!/\sxmlns="/.test(tag)) tag = tag.replace('<svg', '<svg xmlns="http://www.w3.org/2000/svg"')
  const rect = `<rect x="${box.x - 1}" y="${box.y - 1}" width="${box.width + 2}" height="${box.height + 2}" fill="${background}"/>`
  return { width, height, svg: markup.replace(open, `${tag.slice(0, -1)} width="${width}" height="${height}">${rect}`) }
}

/** PNG of a standalone `svg` at `scale`× its pixel size (2 = retina); decodes it through an `<img>`. */
export function rasterize({ svg, width, height }: { svg: string; width: number; height: number }, scale = 2): Promise<Blob> {
  const { promise, resolve, reject } = Promise.withResolvers<Blob>()
  const img = new Image()
  img.onload = () => {
    const canvas = document.createElement('canvas')
    canvas.width = width * scale
    canvas.height = height * scale
    const ctx = canvas.getContext('2d')
    if (!ctx) return reject(new Error('no 2d canvas context'))
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height)
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('PNG encoding failed'))), 'image/png')
  }
  img.onerror = () => reject(new Error('the svg did not decode'))
  img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
  return promise
}

/** Short stable name for a chart: `diagram-<8 hex of FNV-1a>`; the same source exports to the same file. */
export function diagramName(chart: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < chart.length; i++) h = Math.imul(h ^ chart.charCodeAt(i), 0x01000193)
  return `diagram-${(h >>> 0).toString(16).padStart(8, '0')}`
}
