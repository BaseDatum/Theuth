import { createFileRoute } from '@tanstack/react-router'
import { NotesList } from '@/components/notes/NotesList'

/** `/notes` (59 §7.3, 25 §4): the notes list. */
export const Route = createFileRoute('/notes')({ component: NotesList })
