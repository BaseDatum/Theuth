// 158 §11.1: the pure parts of Settings › Meetings › Templates. Edit works on the file itself (`MeetingTemplate.body`);
// only "New template" needs a body built from a shape — every scalar is JSON-quoted, which YAML reads verbatim (YAML
// 1.2 double-quoted strings are a superset of JSON strings), so names with `:` or `#` survive.

import type { Template as MeetingTemplate } from "@/rpc/bindings/method.meeting.output"

/** The parsed shape a body is built from. */
export type TemplateShape = Pick<MeetingTemplate, 'name' | 'description' | 'sections'>

/** What "New template" opens with: one summary section to edit. */
export const NEW_TEMPLATE: TemplateShape = {
  name: '',
  description: '',
  sections: [{ title: 'Summary', instruction: 'at most 120 words', required: false }],
}

/** The `<slug>.md` body for `meeting_template_put`: front matter only (sections live there), `builtin` never written. */
export function templateBody(t: TemplateShape): string {
  const lines = [`name: ${JSON.stringify(t.name)}`, `description: ${JSON.stringify(t.description)}`, 'sections:']
  for (const s of t.sections) {
    lines.push(`  - title: ${JSON.stringify(s.title)}`)
    if (s.instruction) lines.push(`    instruction: ${JSON.stringify(s.instruction)}`)
    if (s.required) lines.push('    required: true')
  }
  return `---\n${lines.join('\n')}\n---\n`
}

/** File stem for a new template: lowercase ASCII words joined by `-` (accents stripped), or `''` when nothing is left. */
export const slugify = (name: string): string =>
  name
    .toLowerCase()
    .normalize('NFKD')
    .replace(/\p{M}/gu, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
