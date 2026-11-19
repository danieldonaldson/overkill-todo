// Guard for OT-006 Decision 6b: every island (`client:*` in a page or layout)
// has a `TelemetryBoundary` at its root, and the boundary names are exactly the
// backend's island allowlist (the shared fixture). Adding an island without a
// boundary, or without its name in the fixture, fails here.
import { describe, expect, it } from 'vitest'

import fixture from '../../../../../backend/tests/fixtures/client_islands.json'

const astroFiles = import.meta.glob(['/src/pages/**/*.astro', '/src/layouts/**/*.astro'], {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>

const svelteFiles = import.meta.glob('/src/**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>

interface Island {
  usedIn: string
  tag: string
  file: string
}

/** Every `<Component client:…>` with the `.svelte` file it imports. */
function islands(): Island[] {
  const found: Island[] = []
  for (const [path, source] of Object.entries(astroFiles)) {
    for (const [, tag] of source.matchAll(/<([A-Z]\w*)\b[^>]*\sclient:[a-z]+/g)) {
      const imported = new RegExp(`import\\s+${tag}\\s+from\\s+['"]([^'"]+)['"]`).exec(source)
      const spec = imported?.[1] ?? ''
      const file = spec.endsWith('.svelte') ? new URL(spec, `file://${path}`).pathname : spec
      found.push({ usedIn: path, tag, file })
    }
  }
  return found
}

/** The markup after the component's script, if it is one root boundary. */
function rootBoundaryName(source: string): string | null {
  const end = source.lastIndexOf('</script>')
  const markup = (end === -1 ? source : source.slice(end + '</script>'.length)).trim()
  const match = /^<TelemetryBoundary\s+name="([^"]+)"[^>]*>[\s\S]*<\/TelemetryBoundary>$/.exec(
    markup
  )
  return match?.[1] ?? null
}

describe('island boundaries', () => {
  const all = islands()

  it('finds the islands', () => {
    expect(all.length).toBeGreaterThanOrEqual(fixture.islands.length)
  })

  it.each(all.map((i) => [`${i.tag} in ${i.usedIn}`, i] as const))(
    '%s is a local component with a TelemetryBoundary at its root',
    (_label, island) => {
      const source = svelteFiles[island.file]
      expect(source, `${island.tag} must import a local .svelte file`).toBeDefined()
      expect(rootBoundaryName(source), `${island.file} has no root boundary`).not.toBeNull()
    }
  )

  it('names every boundary from the shared island allowlist, and uses every name', () => {
    const names = new Set(all.map((i) => rootBoundaryName(svelteFiles[i.file] ?? '')))
    expect([...names].sort()).toEqual([...fixture.islands].sort())
  })

  it('recognises a component without a root boundary', () => {
    expect(rootBoundaryName('<script></script>\n<div>x</div>')).toBeNull()
    expect(
      rootBoundaryName('<script></script>\n<TelemetryBoundary name="X"><p /></TelemetryBoundary>')
    ).toBe('X')
  })
})
