import { MermaidFence } from '@/components/chat/Mermaid'

/** A `.mmd` artifact (35 §3.2): the fence block in `fill` mode — same pan/zoom, fullscreen and export toolbar. */
export function MermaidView({ text }: { text: string }) {
  return (
    <div className="h-full p-3">
      <MermaidFence code={text} isIncomplete={false} language="mermaid" fill />
    </div>
  )
}
