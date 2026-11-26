// Guard for OT-013 (observability, "Content in a script error"): nothing scrubs
// list names or todo titles out of a script error, so the list code must never
// build an error message from data. Every error it creates has a fixed
// message; one built from a template literal or a concatenation fails here.
// The memory server and the model replay are test-only, so they may.
import { describe, expect, it } from 'vitest'

const sources = import.meta.glob(
  [
    '/src/lib/lists/*.ts',
    '/src/components/dashboard/ListsPanel.svelte',
    '!/src/lib/lists/*.test.ts',
    '!/src/lib/lists/memory-server.ts',
    '!/src/lib/lists/*.replay.ts',
  ],
  { query: '?raw', import: 'default', eager: true }
) as Record<string, string>

/** Each `new …Error(` call's first argument, as written. */
function errorMessages(source: string): string[] {
  return [...source.matchAll(/new\s+\w*Error(?:Exception)?\(\s*([^,)]*)/g)].map((m) => m[1].trim())
}

describe('list code errors', () => {
  it('reads the list engine, store and panel', () => {
    expect(Object.keys(sources).sort()).toEqual([
      '/src/components/dashboard/ListsPanel.svelte',
      '/src/lib/lists/engine.ts',
      '/src/lib/lists/store.svelte.ts',
    ])
  })

  it('never builds an error message from data', () => {
    for (const [path, source] of Object.entries(sources)) {
      for (const message of errorMessages(source)) {
        // A fixed string, or passing on an error it was given as a string.
        const fixed = /^(['"])[^'"`]*\1$/.test(message) || /^String\(error\)?$/.test(message)
        expect(fixed, `${path}: new Error(${message}…)`).toBe(true)
      }
    }
  })

  it('catches a message built from a title', () => {
    expect(errorMessages('throw new Error(`could not save ${todo.title}`)')).toEqual([
      '`could not save ${todo.title}`',
    ])
    expect(/^(['"])[^'"`]*\1$/.test('`could not save ${todo.title}`')).toBe(false)
    expect(errorMessages("new Error('save failed: ' + title)")).toEqual(["'save failed: ' + title"])
    expect(/^(['"])[^'"`]*\1$/.test("'save failed: ' + title")).toBe(false)
  })
})
