import '@xyflow/react/dist/style.css'
import { ReactFlow, ReactFlowProvider } from '@xyflow/react'
import { forwardRef } from 'react'
import type { WhiteboardCanvasHandle, WhiteboardCanvasProps } from './api'

// Seam: an empty React Flow; the binding lands in the next commits.
const Canvas = forwardRef<WhiteboardCanvasHandle, WhiteboardCanvasProps>(function Canvas({ className }, _ref) {
  return (
    <div className={className} style={{ width: '100%', height: '100%' }}>
      <ReactFlowProvider>
        <ReactFlow nodes={[]} edges={[]} proOptions={{ hideAttribution: true }} minZoom={0.1} maxZoom={4} />
      </ReactFlowProvider>
    </div>
  )
})

export default Canvas
