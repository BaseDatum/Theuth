import * as React from "react"
import { cn } from "cn"
import { MicButton, splitLayout, useFieldDictation } from "@/components/ui/dictation"

const FIELD =
  "flex field-sizing-content min-h-16 w-full rounded-md border border-input bg-transparent px-3 py-2 text-base shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 md:text-sm dark:bg-input/30 dark:aria-invalid:ring-destructive/40"

/**
 * shadcn textarea with a dictation mic in its bottom-right corner (`dictate={false}` opts out — the composer
 * has its own); width/flex/margin classes move to the wrapper so the box sizes as the bare textarea did.
 */
function Textarea({ className, dictate = true, ...props }: React.ComponentProps<"textarea"> & { dictate?: boolean }) {
  return dictate && !props.readOnly ? <DictatedTextarea className={className} {...props} /> : <textarea data-slot="textarea" className={cn(FIELD, className)} {...props} />
}

function DictatedTextarea({ className, ref, value, ...props }: React.ComponentProps<"textarea">) {
  const mine = React.useRef<HTMLTextAreaElement>(null)
  const controlled = typeof value === "string"
  const d = useFieldDictation(mine, controlled ? value : undefined)
  const [box, field] = splitLayout(className)
  return (
    <div className={cn("relative flex w-full min-w-0", box)}>
      <textarea
        ref={(el) => {
          mine.current = el
          if (typeof ref === "function") ref(el)
          else if (ref) ref.current = el
        }}
        data-slot="textarea"
        className={cn(FIELD, !d.hidden && "pr-8 pointer-coarse:pr-11", field)}
        {...props}
        value={controlled && d.dictating ? d.shown : value}
        readOnly={d.dictating}
      />
      {!props.disabled && <MicButton d={d} className="absolute right-1 bottom-1" />}
    </div>
  )
}

export { Textarea }
