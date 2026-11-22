import { contextBridge, ipcRenderer } from 'electron'
import type { Api } from '../shared/types'

const api: Api = {
  reviewQueue: () => ipcRenderer.invoke('gh:queue'),
  getPR: (owner, repo, number) => ipcRenderer.invoke('gh:pr', owner, repo, number),
  checkout: (pr) => ipcRenderer.invoke('git:checkout', pr),
  readBase: (owner, repo, sha, path) => ipcRenderer.invoke('git:readBase', owner, repo, sha, path),
  fsStat: (path) => ipcRenderer.invoke('fs:stat', path),
  fsRead: (path) => ipcRenderer.invoke('fs:read', path),
  fsReaddir: (path) => ipcRenderer.invoke('fs:readdir', path),
  startLsp: (lang, root) => ipcRenderer.invoke('lsp:start', lang, root),
  stopLsp: (id) => ipcRenderer.invoke('lsp:stop', id),
  reviewState: (owner, repo, number) => ipcRenderer.invoke('gh:reviewState', owner, repo, number),
  addThread: (state, sha, t) => ipcRenderer.invoke('gh:addThread', state, sha, t),
  reply: (state, sha, threadId, body) => ipcRenderer.invoke('gh:reply', state, sha, threadId, body),
  addPRComment: (prNodeId, body) => ipcRenderer.invoke('gh:addPRComment', prNodeId, body),
  deleteComment: (id) => ipcRenderer.invoke('gh:deleteComment', id),
  setResolved: (id, resolved) => ipcRenderer.invoke('gh:setResolved', id, resolved),
  submitReview: (state, sha, event, body) => ipcRenderer.invoke('gh:submitReview', state, sha, event, body),
  openExternal: (url) => ipcRenderer.invoke('shell:open', url),
  onProgress: (cb) => {
    const listener = (_e: unknown, msg: string) => cb(msg)
    ipcRenderer.on('progress', listener)
    return () => ipcRenderer.off('progress', listener)
  },
  onLspExit: (cb) => {
    const listener = (_e: unknown, info: Parameters<typeof cb>[0]) => cb(info)
    ipcRenderer.on('lsp:exit', listener)
    return () => ipcRenderer.off('lsp:exit', listener)
  }
}

contextBridge.exposeInMainWorld('api', api)

// MessagePorts can't go through contextBridge; forward them with window.postMessage.
ipcRenderer.on('lsp:port', (e, info: { id: number; lang: string }) => {
  window.postMessage({ type: 'lsp:port', ...info }, '*', e.ports)
})
