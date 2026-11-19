#!/usr/bin/env node
// Turn a minified browser stack from a `client_error` report back into source
// locations (OT-006 Decision 10). The site never serves its source maps; they
// are kept per release:
//
//   dev        apps/web/dist (a local `pnpm build` keeps the maps there)
//   <sha>      the CI artifact `web-sourcemaps-<sha>`, fetched once with
//              `gh run download` into ~/.cache/overkill-todo/sourcemaps/<sha>
//
// Usage: just decode-stack <release> [file]   (the stack is read from stdin
//        without a file). `--maps <dir>` uses a directory of maps instead.
//
// Each frame `…/<name>.js:<line>:<column>` is looked up in `<name>.js.map`
// anywhere under the maps directory (chunk names carry a content hash, so they
// are unique) and printed with its original file, line, column and name.
// Frames that can't be mapped are printed unchanged.

import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { SourceMapConsumer } from 'source-map'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function usage(message) {
  if (message) console.error(`decode-stack: ${message}`)
  console.error('usage: decode-stack <release> [file] | decode-stack --maps <dir> [file]')
  process.exit(2)
}

/** Every `*.map` under `dir`, by file name. */
function indexMaps(dir) {
  const maps = new Map()
  const walk = (d) => {
    for (const entry of readdirSync(d)) {
      const path = join(d, entry)
      if (statSync(path).isDirectory()) walk(path)
      else if (entry.endsWith('.map')) maps.set(entry, path)
    }
  }
  walk(dir)
  return maps
}

/** The release's maps directory, downloading the CI artifact if needed. */
function mapsFor(release) {
  if (release === 'dev') {
    const dist = join(repo, 'apps', 'web', 'dist')
    if (!existsSync(dist)) usage('no apps/web/dist: run `pnpm --filter @overkill-todo/web build`')
    return dist
  }
  if (!/^[0-9A-Za-z._-]{1,64}$/.test(release)) usage(`not a release: ${release}`)
  const cache = join(
    process.env.XDG_CACHE_HOME ?? join(homedir(), '.cache'),
    'overkill-todo',
    'sourcemaps',
    release
  )
  if (!existsSync(cache)) {
    mkdirSync(cache, { recursive: true })
    try {
      execFileSync(
        'gh',
        ['run', 'download', '--name', `web-sourcemaps-${release}`, '--dir', cache],
        {
          stdio: ['ignore', 'inherit', 'inherit'],
        }
      )
    } catch {
      rmSync(cache, { recursive: true, force: true })
      usage(
        `could not download web-sourcemaps-${release} (needs \`gh auth login\`; artifacts are kept 90 days)`
      )
    }
  }
  return cache
}

/** A frame location: `<url or path>/<name>.js:<line>:<column>` (after `@` in Firefox). */
const FRAME = /([^\s()@]+\.m?js):(\d+):(\d+)/g

/** Show a source path from its repo-relative part (`src/…` or a package). */
function shortSource(mapPath, source) {
  const absolute = resolve(dirname(mapPath), source)
  const fromRepo = relative(join(repo, 'apps', 'web'), absolute)
  if (!fromRepo.startsWith('..')) return fromRepo
  const marker = source.search(/(?:^|\/)(src|node_modules)\//)
  return marker === -1 ? source : source.slice(marker).replace(/^\//, '')
}

async function decode(stack, mapsDir) {
  const maps = indexMaps(mapsDir)
  const consumers = new Map()
  const consumerFor = async (name) => {
    if (!consumers.has(name)) {
      const path = maps.get(`${name}.map`)
      consumers.set(
        name,
        path
          ? { path, consumer: await new SourceMapConsumer(JSON.parse(readFileSync(path, 'utf8'))) }
          : null
      )
    }
    return consumers.get(name)
  }

  const lines = []
  for (const line of stack.split('\n')) {
    let out = ''
    let last = 0
    for (const match of line.matchAll(FRAME)) {
      const [whole, location, lineNo, column] = match
      const found = await consumerFor(location.slice(location.lastIndexOf('/') + 1))
      out += line.slice(last, match.index)
      last = match.index + whole.length
      if (!found) {
        out += whole
        continue
      }
      // Browser columns are 1-based; source maps use 0-based columns.
      const pos = found.consumer.originalPositionFor({
        line: Number(lineNo),
        column: Math.max(0, Number(column) - 1),
      })
      if (pos.source === null) {
        out += whole
        continue
      }
      const where = `${shortSource(found.path, pos.source)}:${pos.line}:${pos.column + 1}`
      out += pos.name ? `${where} (${pos.name})` : where
    }
    lines.push(out + line.slice(last))
  }
  for (const found of consumers.values()) found?.consumer.destroy()
  return lines.join('\n')
}

async function main() {
  const args = process.argv.slice(2)
  let mapsDir
  if (args[0] === '--maps') {
    if (!args[1]) usage('--maps needs a directory')
    mapsDir = resolve(args[1])
    args.splice(0, 2)
  } else {
    if (!args[0]) usage()
    mapsDir = mapsFor(args.shift())
  }
  if (args[0] && !existsSync(args[0])) usage(`no such file: ${args[0]}`)
  if (!args[0] && process.stdin.isTTY) usage('paste a stack on stdin, or give a file')
  const stack = readFileSync(args[0] ?? 0, 'utf8')
  process.stdout.write(`${await decode(stack, mapsDir)}\n`)
}

await main()
