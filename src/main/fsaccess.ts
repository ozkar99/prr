import { readdir, readFile, realpath, stat } from 'node:fs/promises'
import { sep } from 'node:path'
import type { FileStat } from '../shared/types'
import { run } from './exec'
import { worktreesDir } from './git'

/**
 * The editor may read files from the PR worktrees plus the places
 * go-to-definition can lead to: the Go standard library and module cache, and
 * system / Homebrew C++ headers. Everything else is refused.
 */
let roots: string[] | undefined

async function allowedRoots(): Promise<string[]> {
  if (!roots) {
    const goDirs = await run('go', ['env', 'GOROOT', 'GOMODCACHE'])
      .then((out) => out.split('\n').map((l) => l.trim()).filter(Boolean))
      .catch(() => [])
    const candidates = [
      worktreesDir(),
      ...goDirs,
      '/usr/include',
      '/Library/Developer/CommandLineTools',
      '/Applications/Xcode.app/Contents/Developer',
      '/opt/homebrew/include',
      '/opt/homebrew/opt',
      '/opt/homebrew/Cellar'
    ]
    roots = (await Promise.all(candidates.map((p) => realpath(p).catch(() => null)))).filter((p): p is string => !!p)
  }
  return roots
}

async function checked(path: string): Promise<string> {
  const real = await realpath(path)
  const ok = (await allowedRoots()).some((root) => real === root || real.startsWith(root + sep))
  if (!ok) throw new Error(`Access outside allowed folders: ${path}`)
  return real
}

export async function fsStat(path: string): Promise<FileStat | null> {
  try {
    const s = await stat(await checked(path))
    return { type: s.isDirectory() ? 'directory' : 'file', size: s.size, mtime: s.mtimeMs, ctime: s.ctimeMs }
  } catch {
    return null
  }
}

export async function fsRead(path: string): Promise<Uint8Array> {
  return new Uint8Array(await readFile(await checked(path)))
}

export async function fsReaddir(path: string): Promise<[string, 'file' | 'directory'][]> {
  const entries = await readdir(await checked(path), { withFileTypes: true })
  return entries.map((e) => [e.name, e.isDirectory() ? 'directory' : 'file'])
}
