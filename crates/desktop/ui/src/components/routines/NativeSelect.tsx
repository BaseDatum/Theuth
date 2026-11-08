export function NativeSelect({ id, value, onChange, options, none }: { id: string; value: string; onChange(v: string): void; options: readonly (string | { value: string; label: string })[]; none?: string }) {
  return (
    <select id={id} className="h-8 rounded-md border border-input bg-transparent px-2 text-sm" value={value} onChange={(e) => onChange(e.target.value)}>
      {none !== undefined && <option value="">{none}</option>}
      {options.map((o) => {
        const { value, label } = typeof o === 'string' ? { value: o, label: o } : o
        return (
          <option key={value} value={value}>
            {label}
          </option>
        )
      })}
    </select>
  )
}
