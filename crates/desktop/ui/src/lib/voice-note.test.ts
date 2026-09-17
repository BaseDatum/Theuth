import { expect, test } from 'vitest'
import { extFor } from './files'
import { noteText, parseNote } from './voice-note'

test('marker round-trips and matches the harness rounding (45 §4.4)', () => {
  expect(noteText('hi', 8700)).toBe('[voice note, 9 s] hi')
  expect(noteText('hi', 0)).toBe('[voice note, 1 s] hi')
  expect(parseNote('[voice note, 9 s] hi there')).toEqual({ secs: 9, body: 'hi there' })
  expect(parseNote('  [voice note, 12 s]')).toEqual({ secs: 12, body: '' })
  expect(parseNote('typed [voice note, 9 s]')).toBeNull()
})

test('recorder blobs upload under the container extension the harness names them by', () => {
  const ext = (type: string) => extFor(new Blob([], { type }))
  expect(ext('audio/mp4')).toBe('m4a')
  expect(ext('audio/webm;codecs=opus')).toBe('webm')
  expect(ext('audio/ogg;codecs=opus')).toBe('ogg')
  expect(ext('audio/mpeg')).toBe('mp3')
  expect(ext('audio/wav')).toBe('wav')
  expect(ext('image/jpeg')).toBe('jpg')
})
