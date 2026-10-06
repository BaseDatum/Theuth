/**
 * An MCP name split after every underscore run — `argocd_prod_dr__get_application_resource_tree` →
 * `argocd_`, `prod_`, `dr__`, `get_`, … — so the row can put a `<wbr/>` between segments: one unbroken word wraps
 * at its underscores instead of one letter per line on a phone (113). No underscores: the whole name, one segment.
 */
export function segments(name: string): string[] {
  return name.split(/(?<=_)(?!_)/)
}
