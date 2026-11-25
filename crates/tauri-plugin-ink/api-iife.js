if ('__TAURI__' in window) {
  var __TAURI_PLUGIN_INK__ = (function (core) {
    'use strict'
    const call = (cmd, args) => core.invoke('plugin:ink|' + cmd, args)
    return {
      attach(onStrokes, onEvent) {
        const strokes = new core.Channel()
        strokes.onmessage = onStrokes
        const events = new core.Channel()
        events.onmessage = onEvent
        return call('attach', { onStrokes: strokes, onEvent: events })
      },
      detach: () => call('detach', {}),
      setEnabled: (enabled) => call('set_enabled', { enabled }),
      setViewport: (view) => call('set_viewport', { tx: view.tx, ty: view.ty, zoom: view.zoom }),
      setTool: (tool) => call('set_tool', { tool: tool.tool, colour: tool.colour, width: tool.width }),
      setPickerVisible: (visible) => call('set_picker_visible', { visible }),
      ack: (seqs) => call('ack', { seqs }),
      recognizeText: (png) => call('recognize_text', { png }),
    }
  })(window.__TAURI__.core)
  Object.defineProperty(window.__TAURI__, 'ink', { value: __TAURI_PLUGIN_INK__ })
}
