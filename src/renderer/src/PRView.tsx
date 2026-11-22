import { useEffect, useRef, useState } from 'react'
import type { Checkout, LspLanguage, PRDetails, PRFile, ReviewThread } from '../../shared/types'
import { EditorHost, type Location } from './editor/EditorHost'
import { languagesFor, startLanguageClient, type LanguageClientHandle } from './editor/lsp'
import { CommentController } from './review/CommentController'
import { ReviewPanel } from './review/ReviewPanel'
import { Thread } from './review/ThreadZone'
import { pendingCount, ReviewStore, useReview } from './review/ReviewStore'

type LspState = 'starting' | 'running' | { failed: string }

const statusLetter: Record<string, string> = { added: 'A', removed: 'D', modified: 'M', renamed: 'R', copied: 'C', changed: 'M' }

export function PRView({ pr, checkout, onClose }: { pr: PRDetails; checkout: Checkout; onClose: () => void }) {
  const containerRef = useRef<HTMLDivElement>(null)
  const hostRef = useRef<EditorHost>(undefined)
  const [location, setLocation] = useState<Location>()
  const [canGoBack, setCanGoBack] = useState(false)
  const [lsp, setLsp] = useState<Partial<Record<LspLanguage, LspState>>>({})
  const [store] = useState(() => new ReviewStore(pr))
  const review = useReview(store)
  const [panelOpen, setPanelOpen] = useState(false)
  const [showOutdated, setShowOutdated] = useState(false)

  useEffect(() => {
    void store.load()
    const comments = new CommentController(store)
    const host = new EditorHost(
      containerRef.current!,
      pr,
      checkout,
      (loc, back) => {
        setLocation(loc)
        setCanGoBack(back)
      },
      comments
    )
    hostRef.current = host
    if (import.meta.env.DEV) (window as unknown as { __host: EditorHost }).__host = host
    const first = pr.files.find((f) => f.status !== 'removed') ?? pr.files[0]
    if (first) void host.openDiff(first)

    // Servers this view started; cleanup stops only these, so a remount never
    // kills the next view's servers.
    let unmounted = false
    const handles: LanguageClientHandle[] = []
    for (const lang of languagesFor(pr.files.map((f) => f.path))) {
      setLsp((s) => ({ ...s, [lang]: 'starting' }))
      startLanguageClient(lang, checkout.root)
        .then((handle) => {
          if (unmounted) return void handle.dispose()
          handles.push(handle)
          return handle.ready.then(() => !unmounted && setLsp((s) => ({ ...s, [lang]: 'running' })))
        })
        .catch((e: Error) => !unmounted && setLsp((s) => ({ ...s, [lang]: { failed: e.message } })))
    }
    const offExit = window.api.onLspExit(({ id, lang, code, stderrTail }) => {
      if (!handles.some((h) => h.id === id)) return
      setLsp((s) => ({ ...s, [lang]: { failed: `exited (${code ?? 'signal'})\n${stderrTail}` } }))
    })

    return () => {
      unmounted = true
      offExit()
      for (const h of handles) void h.dispose()
      host.dispose()
      hostRef.current = undefined
    }
  }, [pr, checkout, store])

  // Ctrl+- and the mouse back button go back, like VS Code.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === '-') {
        e.preventDefault()
        void hostRef.current?.back()
      }
    }
    const onMouse = (e: MouseEvent) => {
      if (e.button === 3) void hostRef.current?.back()
    }
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('mouseup', onMouse)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      window.removeEventListener('mouseup', onMouse)
    }
  }, [])

  const currentPath =
    location?.kind === 'diff' ? location.file.path : location ? (hostRef.current?.relativePath(location.path) ?? location.path) : ''
  const currentPrFile = location?.kind === 'file' ? pr.files.find((f) => f.path === currentPath) : undefined
  const pending = pendingCount(review.state)
  const threadsByPath = new Map<string, ReviewThread[]>()
  for (const t of review.state?.threads ?? []) threadsByPath.set(t.path, [...(threadsByPath.get(t.path) ?? []), t])
  // Outdated threads have no current line, so they're listed above the editor instead of inline.
  const outdated = location?.kind === 'diff' ? (threadsByPath.get(location.file.path) ?? []).filter((t) => t.line === null) : []

  return (
    <div className="pr-view">
      <header className="titlebar pr-header">
        <button className="link" onClick={onClose}>
          ← Pull requests
        </button>
        <span className="pr-title">
          <span className="muted">
            {pr.owner}/{pr.repo}#{pr.number}
          </span>{' '}
          {pr.title}
        </span>
        <span className="spacer" />
        {Object.entries(lsp).map(([lang, state]) => (
          <span
            key={lang}
            className={`chip ${typeof state === 'object' ? 'chip-error' : state}`}
            title={typeof state === 'object' ? state.failed : undefined}
          >
            {lang === 'go' ? 'gopls' : 'clangd'}: {typeof state === 'object' ? 'failed' : state}
          </span>
        ))}
        <button className="link" onClick={() => void window.api.openExternal(pr.url)}>
          Open on GitHub
        </button>
        <div className="review-button-wrap">
          <button className="button primary" onClick={() => setPanelOpen(!panelOpen)} disabled={!review.state}>
            {pending > 0 ? `Finish review (${pending})` : 'Review'}
          </button>
          {panelOpen && review.state && (
            <ReviewPanel store={store} state={review.state} prAuthor={pr.author} onClose={() => setPanelOpen(false)} />
          )}
        </div>
      </header>

      <aside className="sidebar">
        <div className="sidebar-title">
          Changed files <span className="muted">({pr.files.length})</span>
        </div>
        {review.error && <div className="error sidebar-error">Couldn't load comments: {review.error}</div>}
        <ul className="files">
          {pr.files.map((f) => (
            <FileRow
              key={f.path}
              file={f}
              threads={threadsByPath.get(f.path) ?? []}
              active={location?.kind === 'diff' && location.file.path === f.path}
              onClick={() => void hostRef.current?.openDiff(f)}
            />
          ))}
        </ul>
      </aside>

      <main className="editor-pane">
        <div className="breadcrumb">
          <button className="link" disabled={!canGoBack} onClick={() => void hostRef.current?.back()} title="Back (Ctrl+-)">
            ←
          </button>
          <span className="path">{currentPath}</span>
          {location?.kind === 'file' && !currentPrFile && <span className="muted">not changed in this PR</span>}
          {currentPrFile && (
            <button className="link" onClick={() => void hostRef.current?.openDiff(currentPrFile)}>
              Show diff
            </button>
          )}
          <span className="spacer" />
          {outdated.length > 0 && (
            <button className="link" onClick={() => setShowOutdated(!showOutdated)}>
              {showOutdated ? 'Hide' : 'Show'} {outdated.length} outdated thread{outdated.length === 1 ? '' : 's'}
            </button>
          )}
        </div>
        {showOutdated && outdated.length > 0 && (
          <div className="outdated-panel">
            {outdated.map((t) => (
              <Thread key={t.id} store={store} thread={t} />
            ))}
          </div>
        )}
        <div className="editor" ref={containerRef} />
      </main>
    </div>
  )
}

function FileRow({ file, threads, active, onClick }: { file: PRFile; threads: ReviewThread[]; active: boolean; onClick: () => void }) {
  const slash = file.path.lastIndexOf('/')
  const open = threads.filter((t) => !t.isResolved && !t.isOutdated).length
  const outdated = threads.filter((t) => t.isOutdated).length
  return (
    <li>
      <button className={active ? 'active' : ''} onClick={onClick} title={file.previousPath ? `${file.previousPath} → ${file.path}` : file.path}>
        <span className={`status status-${file.status}`}>{statusLetter[file.status] ?? '?'}</span>
        <span className="name">{file.path.slice(slash + 1)}</span>
        <span className="dir">{file.path.slice(0, slash + 1)}</span>
        {threads.length > 0 && (
          <span
            className={`comment-count ${open ? 'open' : ''}`}
            title={`${threads.length} thread(s): ${open} open${outdated ? `, ${outdated} outdated (not shown inline)` : ''}`}
          >
            💬 {open || threads.length}
          </span>
        )}
        <span className="stats">
          <span className="add">+{file.additions}</span> <span className="del">−{file.deletions}</span>
        </span>
      </button>
    </li>
  )
}
