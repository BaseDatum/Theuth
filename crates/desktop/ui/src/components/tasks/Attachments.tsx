import { AlertCircle, LoaderCircle, Paperclip, Plus, RotateCw, Upload, X } from 'lucide-react'
import { type ClipboardEvent, type DragEvent, useEffect, useRef, useState } from 'react'
import { AttachChip } from '@/components/chat/AttachChip'
import { AttachPicker } from '@/components/chat/AttachPicker'
import { uploadName } from '@/components/chat/uploads'
import { errorText } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { type AttachRef, addRefs, droppedRef, fileMeta, key, normalizeFileAttachId } from '@/lib/attach'
import { extFor, putAttachment, seedFile } from '@/lib/files'
import { notify as toast } from '@/lib/notify'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'

/** Files on a paste event (mirrors Composer's helper without pulling the full chat composer into task views). */
export function pastedFiles(data: { items?: ArrayLike<Pick<DataTransferItem, 'kind' | 'getAsFile'>>; files?: ArrayLike<File> }): File[] {
  const fromItems = Array.from(data.items ?? [])
    .filter((i) => i.kind === 'file')
    .map((i) => i.getAsFile())
    .filter((f): f is File => f !== null)
  return fromItems.length > 0 ? fromItems : Array.from(data.files ?? [])
}

interface PendingUpload {
  id: string
  file: Blob & { name?: string }
  name: string
  status: 'pending' | 'error'
  error?: string
}

/** Build a canonical `'file'` AttachRef from an uploaded engine path and its source File/Blob. */
export function fileToAttachRef(uploadedPath: string, file: Blob & { name?: string }): AttachRef {
  const id = normalizeFileAttachId(uploadedPath)
  const ext = extFor(file)
  const title = file.name || (ext ? `${uploadName(file)}.${ext}` : uploadName(file))
  const metaName = file.name || (ext ? `file.${ext}` : 'file')
  return { kind: 'file', id, title, meta: fileMeta({ name: metaName, size: file.size }) }
}

/**
 * A task's attachments (70 TASK-ATTACHMENTS): supports BOTH arbitrary local files (unrestricted picker, OS drag-drop,
 * clipboard paste via `put_attachment`) and internal theuth items (notes, chats, artifacts, meetings, knowledge).
 * `onChange` gets the whole new list — create mode keeps it in state, edit mode sends `task_update {attachments}`.
 */
export function AttachmentsEditor({
  value,
  onChange,
  disabled,
  onBusyChange,
  className,
}: {
  value: readonly AttachRef[]
  onChange(next: AttachRef[]): void
  disabled?: boolean
  onBusyChange?(busy: boolean): void
  className?: string
}) {
  const box = useRef<HTMLDivElement>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const [picking, setPicking] = useState(false)
  const [over, setOver] = useState(false)
  const [uploads, setUploads] = useState<PendingUpload[]>([])
  const seq = useRef(0)
  const cancelled = useRef(new Set<string>())
  const valueRef = useRef(value)
  const onChangeRef = useRef(onChange)
  valueRef.current = value
  onChangeRef.current = onChange

  const canUpload = useQuerySupport(['put_attachment'])
  const uploading = uploads.some((u) => u.status === 'pending')

  useEffect(() => {
    onBusyChange?.(uploading)
  }, [uploading, onBusyChange])

  useEffect(() => () => {
    for (const id of cancelled.current) cancelled.current.delete(id)
    onBusyChange?.(false)
  }, [onBusyChange])

  const runUpload = (entryId: string, file: Blob & { name?: string }) => {
    const api = useConnection.getState().api
    if (!api) {
      const msg = 'Not connected'
      toast.error(msg)
      setUploads((list) => list.map((u) => (u.id === entryId ? { ...u, status: 'error', error: msg } : u)))
      return
    }
    const ext = extFor(file)
    putAttachment(api, file, ext).then(
      (uploadedPath) => {
        if (cancelled.current.has(entryId)) return
        const ref = fileToAttachRef(uploadedPath, file)
        seedFile(uploadedPath, file)
        seedFile(ref.id, file)
        setUploads((list) => list.filter((u) => u.id !== entryId))
        const next = addRefs(valueRef.current, [ref])
        valueRef.current = next
        onChangeRef.current(next)
      },
      (err: unknown) => {
        if (cancelled.current.has(entryId)) return
        const msg = errorText(err)
        toast.error(msg)
        setUploads((list) => list.map((u) => (u.id === entryId ? { ...u, status: 'error', error: msg } : u)))
      },
    )
  }

  const enqueueFiles = (files: readonly (Blob & { name?: string })[]) => {
    if (disabled || !canUpload || files.length === 0) return
    const added: PendingUpload[] = files.map((file) => ({
      id: `tu-${++seq.current}`,
      file,
      name: uploadName(file),
      status: 'pending',
    }))
    setUploads((list) => [...list, ...added])
    for (const entry of added) runUpload(entry.id, entry.file)
  }

  const cancelUpload = (id: string) => {
    cancelled.current.add(id)
    setUploads((list) => list.filter((u) => u.id !== id))
  }

  const retryUpload = (entry: PendingUpload) => {
    cancelled.current.delete(entry.id)
    setUploads((list) => list.map((u) => (u.id === entry.id ? { ...u, status: 'pending', error: undefined } : u)))
    runUpload(entry.id, entry.file)
  }

  const onDrop = (e: DragEvent) => {
    setOver(false)
    const ref = droppedRef(e.dataTransfer)
    if (ref) {
      e.preventDefault()
      if (!disabled) {
        const next = addRefs(valueRef.current, [ref])
        valueRef.current = next
        onChange(next)
      }
      return
    }
    const files = Array.from(e.dataTransfer.files ?? [])
    if (files.length > 0) {
      e.preventDefault()
      enqueueFiles(files)
    }
  }

  const onPaste = (e: ClipboardEvent) => {
    if (disabled || !canUpload) return
    const files = pastedFiles(e.clipboardData)
    if (files.length === 0) return
    e.preventDefault()
    enqueueFiles(files)
  }

  return (
    <div
      ref={box}
      className={cn(
        'flex flex-wrap items-center gap-1.5 rounded-md transition-shadow',
        over && 'bg-primary/5 ring-2 ring-primary/40',
        className,
      )}
      onDragOver={(e) => {
        e.preventDefault()
        setOver(true)
      }}
      onDragLeave={() => setOver(false)}
      onDrop={onDrop}
      onPaste={onPaste}
    >
      <input
        ref={fileInput}
        type="file"
        multiple
        className="hidden"
        aria-hidden="true"
        tabIndex={-1}
        onChange={(e) => {
          const files = Array.from(e.currentTarget.files ?? [])
          e.currentTarget.value = ''
          enqueueFiles(files)
        }}
      />
      <AttachmentChips
        items={value}
        onRemove={disabled ? undefined : (r) => onChange(value.filter((x) => key(x) !== key(r)))}
      />
      {uploads.map((u) => {
        const failed = u.status === 'error'
        return (
          <span
            key={u.id}
            data-upload={u.status}
            role="status"
            title={failed ? u.error : u.name}
            className={cn(
              'inline-flex h-7 max-w-[240px] items-center gap-1.5 rounded-full border px-2 text-xs',
              failed ? 'border-danger/60 bg-danger-bg/40 text-danger' : 'border-border-subtle bg-layer-1 text-fg-secondary',
            )}
          >
            {failed ? (
              <AlertCircle className="size-3.5 shrink-0" />
            ) : (
              <LoaderCircle className="size-3.5 shrink-0 animate-spin" />
            )}
            <span className="truncate">{u.name}</span>
            {failed && (
              <button
                type="button"
                aria-label={`Retry ${u.name}`}
                className="inline-flex items-center gap-0.5 font-medium hover:underline"
                onClick={() => retryUpload(u)}
              >
                <RotateCw className="size-3" /> Retry
              </button>
            )}
            <button
              type="button"
              aria-label={`Remove ${u.name}`}
              className="-mr-1 flex size-5 shrink-0 items-center justify-center rounded-full text-fg-tertiary hover:bg-layer-2 hover:text-foreground"
              onClick={() => cancelUpload(u.id)}
            >
              <X className="size-3" />
            </button>
          </span>
        )
      })}
      {canUpload && (
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="h-7 px-2 text-xs"
          disabled={disabled}
          onClick={() => fileInput.current?.click()}
          aria-label="Add file"
        >
          <Upload className="size-3.5" /> Add file…
        </Button>
      )}
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className="h-7 px-2 text-xs"
        disabled={disabled}
        onClick={() => setPicking(true)}
        aria-label="Attach note or chat"
      >
        <Plus className="size-3.5" /> Note / Chat…
      </Button>
      {value.length === 0 && uploads.length === 0 && (
        <span className="text-xs text-fg-tertiary">drop or paste any file, note, chat or artifact</span>
      )}
      <AttachPicker
        session={null}
        open={picking}
        anchor={box}
        onClose={() => setPicking(false)}
        onAttach={(refs) => {
          const next = addRefs(valueRef.current, refs)
          valueRef.current = next
          onChange(next)
        }}
      />
    </div>
  )
}

/** The chips alone (read-only unless `onRemove`): what the detail page and the task modal show. */
export function AttachmentChips({ items, onRemove }: { items: readonly AttachRef[]; onRemove?: (r: AttachRef) => void }) {
  return (
    <>
      {items.map((r) => (
        <AttachChip key={key(r)} item={r} onRemove={onRemove && (() => onRemove(r))} />
      ))}
    </>
  )
}

/** The board card's paperclip count (Paperclip icon + N), nothing when empty. */
export function AttachmentCount({ items }: { items: readonly AttachRef[] }) {
  if (items.length === 0) return null
  return (
    <Badge variant="outline" className="text-fg-secondary" aria-label={`${items.length} attachment${items.length === 1 ? '' : 's'}`}>
      <Paperclip /> {items.length}
    </Badge>
  )
}
