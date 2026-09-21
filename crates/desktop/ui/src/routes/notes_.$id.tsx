import { createFileRoute } from '@tanstack/react-router'
import { NotePage } from '@/components/notes/NotePage'

/** `/notes/$id` (59 §7.3): one note in BlockNote with its chat panel; `$id` is the DocId. */
export const Route = createFileRoute('/notes_/$id')({ component: NotePage })
