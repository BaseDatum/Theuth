/**
 * 171 §8: a run's nbformat outputs through the vendored nteract output list. Text, images and JSON render in the
 * main DOM; `text/html`, `image/svg+xml` and `text/markdown` only ever reach 120's sandboxed frame (`HtmlPage`,
 * never `allow-same-origin`, §8.2); anything else falls back to the bundle's `text/plain` (plotly, widgets: wave 3).
 */
import { marked } from 'marked'
import { type ReactNode, useEffect, useState } from 'react'
import { HtmlPage } from '@/components/chat/HtmlPage'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { CellOutput } from '@/rpc/bindings/CellOutput'
import type { CellRunId } from '@/rpc/bindings/CellRunId'
import { type JupyterOutput, OutputArea } from './vendor/nteract/cell/OutputArea'
import { AnsiText as AnsiInline } from './vendor/nteract/outputs/ansi-output'
import { ImageOutput } from './vendor/nteract/outputs/image-output'
import type { CustomRenderer } from './vendor/nteract/outputs/media-router'

export { CellContainer } from './vendor/nteract/cell/CellContainer'
export { ExecutionCount } from './vendor/nteract/cell/ExecutionCount'
export { CellHeader } from './vendor/nteract/elements/CellHeader'
export { ExecutionStatus } from './vendor/nteract/elements/ExecutionStatus'

/** A run file name in an image bundle (§8.5); anything else is nbformat's inline base64. */
const RUN_FILE = /^img-\d+\.png$/

/** The frame's document for one framed MIME: HTML as is, SVG in a bare page, markdown through `marked`. */
export function frameDocument(mimeType: string, content: string): string {
  if (mimeType === 'image/svg+xml') return `<!doctype html><html><body style="margin:0">${content}</body></html>`
  if (mimeType === 'text/markdown') return marked.parse(content, { async: false })
  return content
}

/** The binding's output → nteract's `JupyterOutput` (one shape, §8.1): the stream name narrowed, metadata as a record. */
export function toJupyterOutput(o: CellOutput): JupyterOutput {
  switch (o.output_type) {
    case 'stream':
      return { output_type: 'stream', name: o.name === 'stderr' ? 'stderr' : 'stdout', text: o.text }
    case 'error':
      return { output_type: 'error', ename: o.ename, evalue: o.evalue, traceback: o.traceback }
    default:
      return {
        output_type: o.output_type,
        data: o.data,
        metadata: typeof o.metadata === 'object' && o.metadata !== null && !Array.isArray(o.metadata) ? o.metadata : {},
      }
  }
}

/** An image the run wrote to a file: fetched once by name, a pulse box while it loads. */
function RunImage({ run, name, mediaType, className }: { run: CellRunId; name: string; mediaType: string; className?: string }) {
  const api = useApi()
  const [data, setData] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    setData(null)
    api.cell_output_file(run, name).then(
      (b64) => live && setData(b64),
      () => live && setData(''),
    )
    return () => {
      live = false
    }
  }, [api, run, name])
  if (data === null) return <div className="my-2 h-24 w-48 animate-pulse rounded-md bg-layer-2" aria-busy="true" />
  if (data === '') return <div className="py-2 text-sm text-fg-secondary">Unable to load {name}</div>
  return <ImageOutput data={data} mediaType={mediaType} className={className} />
}

export function CellOutputs({ run, outputs, className }: { run: CellRunId; outputs: CellOutput[]; className?: string }): ReactNode {
  const renderersFor = (index: number): Record<string, CustomRenderer> => {
    const framed: CustomRenderer = ({ data, mimeType }) => (
      <HtmlPage callId={`cell:${run}:${index}`} html={frameDocument(mimeType, String(data))} title={`Output ${index + 1}`} />
    )
    const image: CustomRenderer = ({ data, mimeType, className: cls }) =>
      typeof data === 'string' && RUN_FILE.test(data) ? (
        <RunImage run={run} name={data} mediaType={mimeType} className={cls} />
      ) : (
        <ImageOutput data={String(data)} mediaType={mimeType} className={cls} />
      )
    return { 'text/html': framed, 'image/svg+xml': framed, 'text/markdown': framed, 'image/png': image, 'image/jpeg': image }
  }
  return <OutputArea outputs={outputs.map(toJupyterOutput)} renderersFor={renderersFor} className={className} />
}

/** Static ANSI text (a finished shell run's cleaned screen, §8.3): `\r` resolved, OSC/C0 stripped, the 16 colours as `--ansi-*`. */
export function AnsiText({ text, stream = 'stdout', className }: { text: string; stream?: 'stdout' | 'stderr'; className?: string }): ReactNode {
  return (
    <pre
      data-stream={stream}
      className={cn(
        'not-prose m-0 overflow-x-auto bg-transparent p-0 font-mono text-sm leading-relaxed whitespace-pre-wrap',
        stream === 'stderr' ? 'text-destructive' : 'text-foreground',
        className,
      )}
    >
      <AnsiInline terminalFidelity>{text}</AnsiInline>
    </pre>
  )
}
