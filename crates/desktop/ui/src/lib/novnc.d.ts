// `@novnc/novnc` ships no types; this is the slice `components/vm/VmDesktop.tsx` uses (noVNC 1.7 `core/rfb.js`).
// The package's `exports` map exposes only the root (`./core/rfb.js`), so import from `@novnc/novnc`.
declare module '@novnc/novnc' {
  export type RfbOptions = {
    shared?: boolean
    credentials?: { username?: string; password?: string; target?: string }
    repeaterID?: string
    wsProtocols?: string[]
  }
  export type RfbEvents = {
    connect: CustomEvent<Record<string, never>>
    disconnect: CustomEvent<{ clean: boolean }>
    credentialsrequired: CustomEvent<{ types: string[] }>
    securityfailure: CustomEvent<{ status: number; reason?: string }>
    clipboard: CustomEvent<{ text: string }>
    bell: CustomEvent<Record<string, never>>
    desktopname: CustomEvent<{ name: string }>
    capabilities: CustomEvent<{ capabilities: { power: boolean } }>
    clippingviewport: CustomEvent<boolean>
    serververification: CustomEvent<{ type: string; publickey?: Uint8Array }>
  }
  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, urlOrChannel: string | WebSocket | RTCDataChannel, options?: RfbOptions)
    viewOnly: boolean
    focusOnClick: boolean
    clipViewport: boolean
    dragViewport: boolean
    scaleViewport: boolean
    resizeSession: boolean
    showDotCursor: boolean
    background: string
    qualityLevel: number
    compressionLevel: number
    readonly capabilities: { power: boolean }
    readonly clippingViewport: boolean
    disconnect(): void
    sendCredentials(creds: NonNullable<RfbOptions['credentials']>): void
    sendCtrlAltDel(): void
    /** `down` omitted = press and release. */
    sendKey(keysym: number, code: string | null, down?: boolean): void
    machineShutdown(): void
    machineReboot(): void
    machineReset(): void
    focus(options?: FocusOptions): void
    blur(): void
    clipboardPasteFrom(text: string): void
    addEventListener<K extends keyof RfbEvents>(type: K, listener: (e: RfbEvents[K]) => void): void
    removeEventListener<K extends keyof RfbEvents>(type: K, listener: (e: RfbEvents[K]) => void): void
  }
}
