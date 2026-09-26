import * as React from "react"
import { cn } from "cn"
import { MicButton, TEXT_TYPES, splitLayout, useFieldDictation } from "@/components/ui/dictation"

const FIELD =
  "h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none selection:bg-primary selection:text-primary-foreground file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40"

/**
 * shadcn input. A text field (`type` text/search, not read-only) carries a dictation mic at its trailing edge
 * (`dictate={false}` opts out — the composer has its own); width/flex/margin classes move to the wrapper so
 * the box sizes as the bare input did.
 */
function Input({ className, type, dictate = true, ...props }: React.ComponentProps<"input"> & { dictate?: boolean }) {
  const text = dictate && TEXT_TYPES[type ?? "text"] === true && !props.readOnly
  return text ? <DictatedInput className={className} type={type} {...props} /> : <input type={type} data-slot="input" className={cn(FIELD, className)} {...props} />
}

function DictatedInput({ className, type, ref, value, ...props }: React.ComponentProps<"input">) {
  const mine = React.useRef<HTMLInputElement>(null)
  const controlled = typeof value === "string"
  const d = useFieldDictation(mine, controlled ? value : undefined)
  const [box, field] = splitLayout(className)
  return (
    <div className={cn("relative flex w-full min-w-0", box)}>
      <input
        ref={(el) => {
          mine.current = el
          if (typeof ref === "function") ref(el)
          else if (ref) ref.current = el
        }}
        type={type}
        data-slot="input"
        className={cn(FIELD, !d.hidden && "pr-7 pointer-coarse:pr-11", field)}
        {...props}
        value={controlled && d.dictating ? d.shown : value}
        readOnly={d.dictating}
      />
      {!props.disabled && <MicButton d={d} className="absolute top-1/2 right-0.5 -translate-y-1/2" />}
    </div>
  )
}

export { Input }
