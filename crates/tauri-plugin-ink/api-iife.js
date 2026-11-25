if ('__TAURI__' in window) {
  var __TAURI_PLUGIN_INK__ = (function (core) {
    'use strict'
    const call = (cmd, args) => core.invoke('plugin:ink|' + cmd, args)
    return {
      pencilListen(onEvent) {
        const events = new core.Channel()
        events.onmessage = onEvent
        return call('pencil_listen', { onEvent: events })
      },
      pencilUnlisten: () => call('pencil_unlisten', {}),
      recognizeText: (png) => call('recognize_text', { png }),
    }
  })(window.__TAURI__.core)
  Object.defineProperty(window.__TAURI__, 'ink', { value: __TAURI_PLUGIN_INK__ })
}
