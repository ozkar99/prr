import { MonacoLanguageClient } from 'monaco-languageclient'
import { BrowserMessageReader, BrowserMessageWriter, CloseAction, ErrorAction } from 'vscode-languageclient/browser'
import * as vscode from 'vscode'
import type { LspLanguage } from '../../../shared/types'

const selectors: Record<LspLanguage, { scheme: string; language: string }[]> = {
  go: [{ scheme: 'file', language: 'go' }],
  cpp: [
    { scheme: 'file', language: 'cpp' },
    { scheme: 'file', language: 'c' }
  ],
  python: [{ scheme: 'file', language: 'python' }],
  starlark: [{ scheme: 'file', language: 'starlark' }],
  shell: [{ scheme: 'file', language: 'shellscript' }]
}

export const serverNames: Record<LspLanguage, string> = {
  go: 'gopls',
  cpp: 'clangd',
  python: 'pyright',
  starlark: 'starpls',
  shell: 'bash-language-server'
}

const STARLARK_FILES = /(^|\/)(BUILD|WORKSPACE|WORKSPACE\.bzlmod|MODULE\.bazel)$|\.(bzl|star|bazel|sky)$/

/** Which language servers a PR needs, based on its changed files. */
export function languagesFor(paths: string[]): LspLanguage[] {
  const langs = new Set<LspLanguage>()
  for (const p of paths) {
    if (p.endsWith('.go')) langs.add('go')
    if (/\.(c|cc|cpp|cxx|h|hh|hpp|hxx|ipp|inl|m|mm)$/.test(p)) langs.add('cpp')
    if (/\.pyi?$/.test(p)) langs.add('python')
    if (STARLARK_FILES.test(p)) langs.add('starlark')
    if (/\.(sh|bash|zsh|ksh)$|(^|\/)\.?(bashrc|bash_profile|zshrc|profile)$/.test(p)) langs.add('shell')
  }
  return [...langs]
}

export type LanguageClientHandle = {
  /** Server id, matches the id in lsp:exit events. */
  id: number
  ready: Promise<void>
  dispose(): Promise<void>
}

/**
 * Starts a server for `root` and connects a language client to its MessagePort. Only `file://` models are sent to the server,
 * so the base side of a diff never confuses it.
 */
/**
 * Flags clangd uses for files missing from compile_commands.json (or when
 * there is none). Guessing the usual include folders resolves most
 * project headers without running the repo's build system.
 */
async function clangdFallbackFlags(root: string): Promise<string[]> {
  const dirs = ['', 'include', 'src', 'lib', 'third_party', 'external']
  const existing = await Promise.all(dirs.map(async (d) => ((await window.api.fsStat(`${root}/${d}`))?.type === 'directory' ? d : null)))
  return ['-std=c++20', ...existing.filter((d): d is string => d !== null).map((d) => `-I${root}${d ? '/' + d : ''}`)]
}

export async function startLanguageClient(lang: LspLanguage, root: string): Promise<LanguageClientHandle> {
  const initializationOptions =
    lang === 'cpp'
      ? { fallbackFlags: await clangdFallbackFlags(root) }
      : lang === 'go'
        ? // Off by default in gopls; VS Code's Go extension turns it on. Colors types, packages, etc.
          { semanticTokens: true }
        : undefined
  const { id, port } = await window.api.startLsp(lang, root)
  let client: MonacoLanguageClient | undefined
  let disposed = false

  const ready = (async () => {
    if (disposed) return
    client = new MonacoLanguageClient({
      name: serverNames[lang],
      id: `${lang}-${id}`,
      clientOptions: {
        documentSelector: selectors[lang],
        initializationOptions,
        workspaceFolder: { index: 0, name: 'pr', uri: vscode.Uri.file(root) },
        errorHandler: {
          error: () => ({ action: ErrorAction.Continue }),
          closed: () => ({ action: CloseAction.DoNotRestart })
        }
      },
      messageTransports: { reader: new BrowserMessageReader(port), writer: new BrowserMessageWriter(port) }
    })
    if (import.meta.env.DEV) ((window as unknown as { __lsp?: Record<string, unknown> }).__lsp ??= {})[lang] = client
    await client.start()
  })()

  return {
    id,
    ready,
    async dispose() {
      if (disposed) return
      disposed = true
      await client?.dispose().catch(() => {})
      await window.api.stopLsp(id)
    }
  }
}
