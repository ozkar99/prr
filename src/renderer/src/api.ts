// window.api on Tauri: the same interface the Electron preload provided, built on invoke/listen.
import { Channel, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { Api, ReviewState } from '../../shared/types'

/** Tauri rejects with the Rust error string; the UI expects Error objects. */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args)
  } catch (e) {
    throw e instanceof Error ? e : new Error(String(e))
  }
}

/** Mutations only need these two fields, not the whole thread list. */
const handle = (s: ReviewState) => ({ prNodeId: s.prNodeId, pendingReviewId: s.pendingReviewId ?? null })

function subscribe<T>(event: string, cb: (payload: T) => void): () => void {
  const unlisten = listen<T>(event, (e) => cb(e.payload))
  return () => void unlisten.then((f) => f())
}

const api: Api = {
  reviewQueue: () => call('review_queue'),
  getPR: (owner, repo, number) => call('get_pr', { owner, repo, number }),
  checkout: (pr) => call('checkout', { pr }),
  readBase: (owner, repo, sha, path) => call('read_base', { owner, repo, sha, path }),
  fsStat: (path) => call('fs_stat', { path }),
  fsRead: async (path) => new Uint8Array(await call<ArrayBuffer>('fs_read', { path })),
  fsReaddir: (path) => call('fs_readdir', { path }),

  async startLsp(lang, root) {
    // Bridge Tauri's Channel (Rust -> page) and lsp_send (page -> Rust) onto a
    // MessagePort, which vscode-languageclient's browser transport speaks.
    const { port1, port2 } = new MessageChannel()
    const onMessage = new Channel<unknown>()
    onMessage.onmessage = (msg) => port2.postMessage(msg)
    const id = await call<number>('lsp_start', { lang, root, onMessage })
    port2.onmessage = (e) => void call('lsp_send', { id, message: e.data }).catch(() => {})
    return { id, port: port1 }
  },
  stopLsp: (id) => call('lsp_stop', { id }),

  reviewState: (owner, repo, number) => call('review_state', { owner, repo, number }),
  addThread: (state, headSha, thread) => call('add_thread', { state: handle(state), headSha, thread }),
  reply: (state, headSha, threadId, body) => call('reply', { state: handle(state), headSha, threadId, body }),
  addPRComment: (prNodeId, body) => call('add_pr_comment', { prNodeId, body }),
  deleteComment: (id) => call('delete_comment', { id }),
  setResolved: (threadId, resolved) => call('set_resolved', { threadId, resolved }),
  submitReview: (state, headSha, event, body) => call('submit_review', { state: handle(state), headSha, event, body }),

  openExternal: (url) => call('open_url', { url }),

  setReviewContext: (review) => call('set_review_context', { review }),
  setSelection: (selection) => call('set_selection', { selection }),
  agentInfo: () => call('agent_info'),
  async agentStart(opts, onData) {
    const channel = new Channel<ArrayBuffer>()
    channel.onmessage = (bytes) => onData(new Uint8Array(bytes))
    return call<number>('agent_start', { ...opts, onData: channel })
  },
  ptyWrite: (id, data) => call('pty_write', { id, data }),
  ptyResize: (id, cols, rows) => call('pty_resize', { id, cols, rows }),
  ptyKill: (id) => call('pty_kill', { id }),
  onPtyExit: (cb) => subscribe('pty:exit', cb),
  takeLaunchPR: () => call('take_launch_pr'),
  onOpenPR: (cb) => subscribe('app:openPR', cb),
  onProgress: (cb) => subscribe('progress', cb),
  onLspExit: (cb) => subscribe('lsp:exit', cb)
}

window.api = api

// Links opened by the editor (hovers, Markdown) go to the default browser.
window.open = ((url?: string | URL) => {
  if (url) void api.openExternal(String(url)).catch(() => {})
  return null
}) as typeof window.open
