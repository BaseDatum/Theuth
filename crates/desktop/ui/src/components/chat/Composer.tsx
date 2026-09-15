import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { open as pickFiles } from '@tauri-apps/plugin-dialog'
import { ArrowUp, Paperclip, Plus, Square, X } from 'lucide-react'
import { type ClipboardEvent, type KeyboardEvent, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Textarea } from '@/components/ui/textarea'
import { attach, expand, type Pastes, prune, token, tokenize } from '@/lib/pastes'
import { queryKeys, useApi } from '@/lib/rpc'
import { isTauri, saveAttachment } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { Attachments } from './Attachments'
import { QueuedChips } from './QueuedChips'
import { type Ctx, FRESH, matches, run } from './slash'

const NONE: Pastes = new Map()
const IMAGE = /\.(png|jpe?g|gif|webp|bmp)$/i

/** The slash popup: commands matching the typed word, one highlighted. */
function SlashList({ word, selected, onPick }: { word: string; selected: number; onPick(name: string): void }) {
  return (
    <div className="absolute bottom-full left-0 mb-2 max-h-72 w-full overflow-y-auto rounded-xl border bg-popover p-1 text-sm shadow-md">
      {matches(word).map((c, i) => (
        <button
          type="button"
          key={c.name}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(c.name)}
          className={cn('flex w-full items-baseline gap-2 rounded-lg px-3 py-2 text-left', i === selected && 'bg-accent')}
        >
          <span className="font-mono">{c.name}</span>
          {c.hint && <span className="font-mono text-fg-tertiary">{c.hint}</span>}
          <span className="ml-auto truncate text-fg-secondary">{c.desc}</span>
        </button>
      ))}
    </div>
  )
}

/**
 * ChatGPT's composer: `+` attach on the left, send / stop on the right, Enter
 * sends, Shift+Enter breaks a line. Large pastes and attachments become
 * `[⎘ …]` tokens (`lib/pastes`) expanded on submit. `/` at column 0 opens
 * the command list; other `/x` runs `use_skill`.
 */
export function Composer({ session, draftEffort }: { session: string | null; draftEffort?: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const running = useSessionField(session ?? '', 'running') ?? false
  const model = useSessionField(session ?? '', 'model')
  const parent = useSessionField(session ?? '', 'parent')
  const defaultModel = useConnection((s) => s.info?.default_model)
  const { data: models } = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const [text, setText] = useState('')
  const [pastes, setPastes] = useState<Pastes>(NONE)
  const [selected, setSelected] = useState(0)
  const [busy, setBusy] = useState(false)
  const ref = useRef<HTMLTextAreaElement>(null)

  const slashWord = /^\/\S*$/.test(text) ? text : null
  const slash = slashWord !== null && matches(slashWord).length > 0
  const files = [...pastes.values()].flatMap((p) => (p.kind === 'file' ? [p.path] : []))

  useEffect(() => {
    const focus = () => ref.current?.focus()
    window.addEventListener('wth:focus-composer', focus)
    return () => window.removeEventListener('wth:focus-composer', focus)
  }, [])

  const insert = (s: string) => {
    const ta = ref.current
    const [a, b] = ta ? [ta.selectionStart, ta.selectionEnd] : [text.length, text.length]
    setText((t) => t.slice(0, a) + s + t.slice(b))
  }

  const addFiles = (paths: string[], dims?: [number, number]) => {
    let next = pastes
    const tokens: string[] = []
    for (const path of paths) {
      const held = attach(path, dims?.[0] ?? 0, dims?.[1] ?? 0, next)
      next = held.pastes
      tokens.push(held.token)
    }
    setPastes(next)
    insert(`${tokens.join(' ')} `)
    const id = model ?? defaultModel
    const info = models?.find((m) => m.id === id)
    if (info && !info.image && paths.some((p) => IMAGE.test(p))) toast.warning(`${info.name} does not accept images`)
  }

  // One native listener for the component's life; it reads the latest handler through the ref.
  const dropRef = useRef(addFiles)
  dropRef.current = addFiles
  useEffect(() => {
    if (!isTauri()) return
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop') dropRef.current(e.payload.paths)
    })
    return () => void un.then((f) => f())
  }, [])

  const onPaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const image = [...e.clipboardData.items].find((i) => i.type.startsWith('image/'))?.getAsFile()
    if (image) {
      e.preventDefault()
      const bitmap = await createImageBitmap(image)
      const ext = image.type === 'image/jpeg' ? 'jpg' : image.type.slice('image/'.length)
      const path = await saveAttachment(new Uint8Array(await image.arrayBuffer()), ext)
      addFiles([path], [bitmap.width, bitmap.height])
      bitmap.close()
      return
    }
    const plain = e.clipboardData.getData('text/plain')
    if (!plain) return
    e.preventDefault()
    const held = tokenize(plain, pastes)
    setPastes(held.pastes)
    insert(held.text)
  }

  const removeFile = (path: string) => {
    for (const [i, p] of pastes) {
      if (p.kind !== 'file' || p.path !== path) continue
      const next = text.replace(token(i, p), '').replace(/ {2,}/g, ' ').trimStart()
      setText(next)
      setPastes(prune(next, pastes))
    }
  }

  const reset = () => {
    setText('')
    setPastes(NONE)
    setSelected(0)
  }

  const ctx: Ctx = {
    api,
    session,
    parent,
    navigate,
    async reload() {
      if (session) useSessions.getState().loadHistory(session, await api.load_session(session, null, 60), 'replace')
    },
  }

  const submit = async () => {
    const line = text.trim()
    if (!line || busy) return
    setBusy(true)
    try {
      if (line.startsWith('/')) {
        await run(line, ctx)
        reset()
        return
      }
      const { text: body, files } = expand(line, pastes)
      const { markSent, applyEvents, loadHistory } = useSessions.getState()
      reset()
      if (session && running) {
        const id = crypto.randomUUID()
        applyEvents([{ type: 'InputQueued', session, id, text: body }])
        await api.steer(session, body, id, files)
      } else if (session) {
        markSent(session, body, files)
        await api.send_input(session, body, files)
      } else {
        let id: string
        if (draftEffort === undefined) {
          id = await api.send_input(null, body, files)
        } else {
          id = await api.new_session()
          await api.set_effort(id, draftEffort)
          await api.send_input(id, body, files)
        }
        // A fresh session has nothing stored yet: mark it loaded so the route does not
        // re-fetch and drop the bubble below (its echo is skipped via `markSent`).
        loadHistory(id, { ...FRESH, model: defaultModel ?? '', effort: draftEffort ?? null }, 'replace')
        markSent(id, body, files)
        await navigate({ to: '/c/$session', params: { session: id } })
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const pick = (name: string) => {
    const hint = matches(name)[0]?.hint ?? ''
    setText(`${name} `)
    setSelected(0)
    if (hint === '') {
      void run(name, ctx)
        .then(reset)
        .catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
    }
  }

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (slash && slashWord) {
      const list = matches(slashWord)
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setSelected((i) => (i + (e.key === 'ArrowDown' ? 1 : list.length - 1)) % list.length)
        return
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
        e.preventDefault()
        const c = list[selected] ?? list[0]
        if (c) pick(c.name)
        return
      }
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault()
      void submit()
    }
  }

  return (
    <div className="mx-auto w-full max-w-[768px] px-4 pb-4">
      {session && <QueuedChips session={session} onEdit={(t) => setText(t)} />}
      <div className="relative rounded-[28px] border border-border-subtle bg-elevated px-2 py-[7px] shadow-sm">
        {slash && slashWord && <SlashList word={slashWord} selected={selected} onPick={pick} />}
        {files.length > 0 && (
          <div className="flex flex-wrap gap-2 px-2 pt-2">
            {files.map((path) => (
              <div key={path} className="relative">
                <Attachments files={[path]} className="[&_img]:max-h-20" />
                <button
                  type="button"
                  aria-label="Remove attachment"
                  onClick={() => removeFile(path)}
                  className="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full bg-foreground text-background"
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="flex items-end gap-1">
          <Popover>
            <PopoverTrigger asChild>
              <Button variant="ghost" className="size-9 shrink-0 rounded-full text-fg-secondary" aria-label="Add">
                <Plus className="size-5" />
              </Button>
            </PopoverTrigger>
            <PopoverContent align="start" className="w-48 p-1">
              <Button
                variant="ghost"
                className="w-full justify-start"
                onClick={async () => {
                  const picked = await pickFiles({
                    multiple: true,
                    filters: [{ name: 'Images and PDF', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'pdf'] }],
                  })
                  if (picked) addFiles(picked)
                }}
              >
                <Paperclip /> Attach file
              </Button>
            </PopoverContent>
          </Popover>
          <Textarea
            ref={ref}
            value={text}
            onChange={(e) => {
              setText(e.target.value)
              setPastes(prune(e.target.value, pastes))
              setSelected(0)
            }}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            rows={1}
            placeholder={running ? 'Send to steer…' : 'Ask anything'}
            // ponytail: `field-sizing: content` grows the box (8 lines then scrolls); older WebKit keeps one line.
            className="max-h-[200px] min-h-10 min-w-0 flex-1 resize-none border-0 bg-transparent px-2 py-2 text-[17px] leading-6 shadow-none focus-visible:ring-0 md:text-[17px] dark:bg-transparent"
          />
          {running && session ? (
            <Button className="size-9 shrink-0 rounded-full" aria-label="Stop" onClick={() => void api.cancel(session)}>
              <Square className="size-4 fill-current" />
            </Button>
          ) : (
            <Button
              className="size-9 shrink-0 rounded-full"
              aria-label="Send"
              disabled={!text.trim() || busy}
              onClick={() => void submit()}
            >
              <ArrowUp className="size-5" />
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
