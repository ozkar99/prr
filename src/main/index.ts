import { app, BrowserWindow, ipcMain, shell } from 'electron'
import { join } from 'node:path'
import { parsePRRef, type LspLanguage, type NewThread, type PRDetails, type PRRef, type ReviewEvent, type ReviewState } from '../shared/types'
import { augmentPath } from './exec'
import { fsRead, fsReaddir, fsStat } from './fsaccess'
import { checkout, readAtCommit } from './git'
import { addPRComment, addThread, deleteComment, getPR, reply, reviewQueue, reviewState, setResolved, submitReview } from './github'
import { startServer, stopAllServers, stopServer } from './lsp'

augmentPath()

/** `review-tool <pr>` launches the app with `--open-pr=<url>`. */
function prFromArgv(argv: string[]): PRRef | undefined {
  const arg = argv.find((a) => a.startsWith('--open-pr='))
  return arg ? parsePRRef(arg.slice('--open-pr='.length)) : undefined
}

// One window for all PRs: a second launch hands its PR to the running app and exits.
if (!app.requestSingleInstanceLock()) {
  app.quit()
  process.exit(0)
}

let launchPR = prFromArgv(process.argv)

app.on('second-instance', (_e, argv) => {
  const win = BrowserWindow.getAllWindows()[0] ?? createWindow()
  if (win.isMinimized()) win.restore()
  win.show()
  app.focus({ steal: true })
  const ref = prFromArgv(argv)
  if (!ref) return
  // If the page is still loading, it will pick this up via takeLaunchPR.
  if (win.webContents.isLoading()) launchPR = ref
  else win.webContents.send('app:openPR', ref)
})

function createWindow(): BrowserWindow {
  const win = new BrowserWindow({
    width: 1500,
    height: 950,
    title: 'Review Tool',
    titleBarStyle: 'hiddenInset',
    backgroundColor: '#1f1f1f',
    webPreferences: {
      preload: join(import.meta.dirname, '../preload/index.mjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  // Links (PR page, hover docs) open in the default browser, never in-app.
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/.test(url)) void shell.openExternal(url)
    return { action: 'deny' }
  })
  // A reload or crash orphans the renderer's language clients; stop their servers.
  const wcId = win.webContents.id
  win.webContents.on('did-start-navigation', (details) => {
    if (details.isMainFrame && !details.isSameDocument) stopAllServers(wcId)
  })
  win.webContents.on('render-process-gone', () => stopAllServers(wcId))
  win.on('closed', () => stopAllServers(wcId))
  win.webContents.on('will-navigate', (e, url) => {
    if (url !== win.webContents.getURL()) e.preventDefault()
  })

  if (process.env.ELECTRON_RENDERER_URL) {
    void win.loadURL(process.env.ELECTRON_RENDERER_URL)
  } else {
    void win.loadFile(join(import.meta.dirname, '../renderer/index.html'))
  }
  return win
}

app.whenReady().then(() => {
  ipcMain.handle('app:takeLaunchPR', () => {
    const ref = launchPR ?? null
    launchPR = undefined
    return ref
  })
  ipcMain.handle('gh:queue', () => reviewQueue())
  ipcMain.handle('gh:pr', (_e, owner: string, repo: string, number: number) => getPR(owner, repo, number))
  ipcMain.handle('gh:reviewState', (_e, owner: string, repo: string, number: number) => reviewState(owner, repo, number))
  ipcMain.handle('gh:addThread', (_e, state: ReviewState, sha: string, t: NewThread) => addThread(state, sha, t))
  ipcMain.handle('gh:reply', (_e, state: ReviewState, sha: string, threadId: string, body: string) => reply(state, sha, threadId, body))
  ipcMain.handle('gh:addPRComment', (_e, prNodeId: string, body: string) => addPRComment(prNodeId, body))
  ipcMain.handle('gh:deleteComment', (_e, id: string) => deleteComment(id))
  ipcMain.handle('gh:setResolved', (_e, id: string, resolved: boolean) => setResolved(id, resolved))
  ipcMain.handle('gh:submitReview', (_e, state: ReviewState, sha: string, event: ReviewEvent, body: string) => submitReview(state, sha, event, body))
  ipcMain.handle('git:checkout', (e, pr: PRDetails) => checkout(pr, (msg) => e.sender.send('progress', msg)))
  ipcMain.handle('git:readBase', (_e, owner: string, repo: string, sha: string, path: string) => readAtCommit(owner, repo, sha, path))
  ipcMain.handle('fs:stat', (_e, path: string) => fsStat(path))
  ipcMain.handle('fs:read', (_e, path: string) => fsRead(path))
  ipcMain.handle('fs:readdir', (_e, path: string) => fsReaddir(path))
  ipcMain.handle('lsp:start', (e, lang: LspLanguage, root: string) =>
    startServer(e.sender, lang, root, (id, code, stderrTail) => {
      if (!e.sender.isDestroyed()) e.sender.send('lsp:exit', { id, lang, code, stderrTail })
    })
  )
  ipcMain.handle('lsp:stop', (_e, id: number) => stopServer(id))
  ipcMain.handle('shell:open', (_e, url: string) => {
    if (/^https:\/\/github\.com\//.test(url)) return shell.openExternal(url)
  })

  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  stopAllServers()
  if (process.platform !== 'darwin') app.quit()
})
app.on('before-quit', () => stopAllServers())
