import { MessageChannelMain, type WebContents } from 'electron'
import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { delimiter, isAbsolute, join } from 'node:path'
import { StreamMessageReader, StreamMessageWriter } from 'vscode-jsonrpc/node'
import type { LspLanguage } from '../shared/types'

type Server = { proc: ChildProcess; webContentsId: number; dispose: () => void }
const servers = new Map<number, Server>()
let nextId = 1

function clangdPath(): string {
  // Prefer Homebrew LLVM (newer) over the Xcode Command Line Tools build.
  const brew = '/opt/homebrew/opt/llvm/bin/clangd'
  return existsSync(brew) ? brew : 'clangd'
}

/** Absolute path of an executable on PATH, or undefined. */
function which(cmd: string): string | undefined {
  if (isAbsolute(cmd)) return existsSync(cmd) ? cmd : undefined
  return (process.env.PATH ?? '')
    .split(delimiter)
    .map((dir) => join(dir, cmd))
    .find((p) => existsSync(p))
}

const installHints: Record<LspLanguage, string> = {
  go: 'go install golang.org/x/tools/gopls@latest',
  cpp: 'xcode-select --install  (or: brew install llvm)',
  python: 'bundled with the app; try reinstalling dependencies (npm install)',
  starlark: 'download starpls from https://github.com/withered-magic/starpls/releases and put it on your PATH'
}

function command(lang: LspLanguage, root: string): { cmd: string; args: string[]; env: NodeJS.ProcessEnv } {
  if (lang === 'python') {
    // pyright ships as an npm package; run it with Electron's built-in Node.
    const langserver = createRequire(import.meta.url).resolve('pyright/langserver.index.js')
    return { cmd: process.execPath, args: [langserver, '--stdio'], env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' } }
  }
  if (lang === 'starlark') {
    return { cmd: 'starpls', args: ['server'], env: process.env }
  }
  if (lang === 'go') {
    // GOTOOLCHAIN=local stops a PR's go.mod from making us download and run a
    // different Go toolchain.
    return { cmd: 'gopls', args: ['serve'], env: { ...process.env, GOTOOLCHAIN: 'local' } }
  }
  const args = ['--background-index', '--header-insertion=never', '--clang-tidy=false', '--pch-storage=memory']
  for (const dir of ['build', 'out', 'cmake-build-debug', 'builddir']) {
    if (existsSync(join(root, dir, 'compile_commands.json'))) {
      args.push(`--compile-commands-dir=${join(root, dir)}`)
      break
    }
  }
  return { cmd: clangdPath(), args, env: process.env }
}

/**
 * Starts a language server for the worktree and hands the renderer a
 * MessagePort that speaks LSP JSON-RPC. Main just relays messages between the
 * port and the server's stdio.
 */
export function startServer(
  wc: WebContents,
  lang: LspLanguage,
  root: string,
  onExit: (id: number, code: number | null, stderrTail: string) => void
): number {
  const id = nextId++

  const { cmd, args, env } = command(lang, root)
  const resolved = which(cmd)
  if (!resolved) throw new Error(`not installed: ${cmd}. To install: ${installHints[lang]}`)
  const proc = spawn(resolved, args, { cwd: root, env, stdio: ['pipe', 'pipe', 'pipe'] })

  let stderrTail = ''
  proc.stderr!.on('data', (chunk: Buffer) => {
    stderrTail = (stderrTail + chunk.toString()).slice(-4000)
  })

  const reader = new StreamMessageReader(proc.stdout!)
  const writer = new StreamMessageWriter(proc.stdin!)
  const { port1, port2 } = new MessageChannelMain()

  reader.listen((msg) => port1.postMessage(msg))
  port1.on('message', (e) => void writer.write(e.data).catch(() => {}))
  port1.start()

  let disposed = false
  const dispose = () => {
    if (disposed) return
    disposed = true
    reader.dispose()
    writer.dispose()
    port1.close()
    if (proc.exitCode === null) proc.kill()
  }

  proc.on('error', (err) => {
    stderrTail += `\n${err.message}`
  })
  proc.on('close', (code) => {
    // Only report exits we didn't ask for.
    const unexpected = servers.delete(id)
    dispose()
    if (unexpected) onExit(id, code, stderrTail)
  })

  servers.set(id, { proc, webContentsId: wc.id, dispose })
  wc.postMessage('lsp:port', { id, lang }, [port2])
  return id
}

export function stopServer(id: number): void {
  const server = servers.get(id)
  servers.delete(id)
  server?.dispose()
}

/** Stops every server, or only those belonging to one window. */
export function stopAllServers(webContentsId?: number): void {
  for (const [id, server] of servers) {
    if (webContentsId === undefined || server.webContentsId === webContentsId) stopServer(id)
  }
}
