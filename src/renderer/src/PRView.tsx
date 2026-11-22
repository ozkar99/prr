import { useEffect, useRef, useState } from 'react'
import type { Checkout, LspLanguage, PRDetails, PRFile, ReviewThread } from '../../shared/types'
import { EditorHost, type Location } from './editor/EditorHost'
import { languagesFor, serverNames, startLanguageClient, type LanguageClientHandle } from './editor/lsp'
import { CommentController } from './review/CommentController'
import { ReviewPanel } from './review/ReviewPanel'
import { Thread } from './review/ThreadZone'
import { Conversation } from './review/Conversation'
import { snippet } from './review/snippet'
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
  const [view, setView] = useState<'code' | 'conversation'>('code')
  /** Conversation item to scroll to; undefined means the top (description). */
  const [convTarget, setConvTarget] = useState<string>()
  const openConversation = (itemId?: string) => {
    setView('conversation')
    setConvTarget(itemId)
    // After the conversation view renders.
    requestAnimationFrame(() => {
      const el = itemId ? document.getElementById(`conv-${itemId}`) : document.querySelector('.conversation-inner')
      el?.scrollIntoView({ block: 'start', behavior: 'smooth' })
      if (itemId) {
        el?.classList.add('flash')
        setTimeout(() => el?.classList.remove('flash'), 1200)
      }
    })
  }

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
  // Threads on files that are no longer part of the diff (e.g. the change was reverted).
  const changedPaths = new Set(pr.files.map((f) => f.path))
  const orphanThreads = (review.state?.threads ?? []).filter((t) => !changedPaths.has(t.path))

  const openFile = (f: PRFile) => {
    setView('code')
    void hostRef.current?.openDiff(f)
  }
  const openThread = (f: PRFile, t: ReviewThread) => {
    setView('code')
    if (t.line === null) {
      setShowOutdated(true)
      void hostRef.current?.openDiff(f)
    } else {
      void hostRef.current?.openDiff(f, { lineNumber: t.line, column: 1 }, t.side)
    }
  }

  return (
    <div className="pr-view">
      <header className="titlebar pr-header" data-tauri-drag-region>
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
            {serverNames[lang as LspLanguage]}: {typeof state === 'object' ? (state.failed.includes('not installed') ? 'not installed' : 'failed') : state}
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
          Conversation <span className="muted">({review.state?.conversation.length ?? '…'})</span>
        </div>
        <ul className="files conversation-list">
          <li>
            <button className={view === 'conversation' && !convTarget ? 'active' : ''} onClick={() => openConversation()}>
              <span className="conv-icon">≡</span>
              <span className="name">Description</span>
              <span className="thread-snippet muted">{snippet(pr.body) || 'No description'}</span>
            </button>
          </li>
          {review.state?.conversation.map((item) => (
            <li key={item.id}>
              <button
                className={`thread-row conv-row ${view === 'conversation' && convTarget === item.id ? 'active' : ''}`}
                onClick={() => openConversation(item.id)}
                title={`${item.author}: ${item.body}`}
              >
                <span className={`conv-icon verdict-${item.reviewState?.toLowerCase() ?? 'comment'}`}>
                  {item.reviewState === 'APPROVED' ? '✓' : item.reviewState === 'CHANGES_REQUESTED' ? '±' : '💬'}
                </span>
                <span className="thread-author">{item.author}</span>
                <span className="thread-snippet">
                  {snippet(item.body) ||
                    (item.reviewState === 'APPROVED' ? 'approved' : item.reviewState === 'CHANGES_REQUESTED' ? 'requested changes' : '')}
                </span>
              </button>
            </li>
          ))}
        </ul>
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
              active={view === 'code' && location?.kind === 'diff' && location.file.path === f.path}
              onClick={() => openFile(f)}
              onThreadClick={(t) => openThread(f, t)}
            />
          ))}
        </ul>
        {orphanThreads.length > 0 && (
          <>
            <div className="sidebar-title">
              Comments on files no longer in the diff <span className="muted">({orphanThreads.length})</span>
            </div>
            <ul className="files">
              {orphanThreads.map((t) => (
                <ThreadRow key={t.id} thread={t} showPath onClick={() => void window.api.openExternal(t.comments[0]?.url ?? pr.url)} />
              ))}
            </ul>
          </>
        )}
      </aside>

      {view === 'conversation' && <Conversation pr={pr} store={store} />}
      <main className="editor-pane" style={view === 'conversation' ? { display: 'none' } : undefined}>
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

function FileRow({
  file,
  threads,
  active,
  onClick,
  onThreadClick
}: {
  file: PRFile
  threads: ReviewThread[]
  active: boolean
  onClick: () => void
  onThreadClick: (t: ReviewThread) => void
}) {
  const [expanded, setExpanded] = useState(true)
  const slash = file.path.lastIndexOf('/')
  const open = threads.filter((t) => !t.isResolved && !t.isOutdated).length
  const outdated = threads.filter((t) => t.isOutdated).length
  // Current threads in line order, then outdated ones.
  const sorted = [...threads].sort((a, b) => (a.line ?? Infinity) - (b.line ?? Infinity))
  return (
    <li>
      <button className={active ? 'active' : ''} onClick={onClick} title={file.previousPath ? `${file.previousPath} → ${file.path}` : file.path}>
        <span
          className="twisty"
          onClick={(e) => {
            if (!threads.length) return
            e.stopPropagation()
            setExpanded(!expanded)
          }}
        >
          {threads.length ? (expanded ? '▾' : '▸') : ''}
        </span>
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
      {expanded && sorted.length > 0 && (
        <ul className="thread-list">
          {sorted.map((t) => (
            <ThreadRow key={t.id} thread={t} onClick={() => onThreadClick(t)} />
          ))}
        </ul>
      )}
    </li>
  )
}

function ThreadRow({ thread, showPath, onClick }: { thread: ReviewThread; showPath?: boolean; onClick: () => void }) {
  const first = thread.comments[0]
  const where = thread.isOutdated || thread.line === null ? 'outdated' : `L${thread.line}${thread.side === 'LEFT' ? ' (old)' : ''}`
  const pending = thread.comments.some((c) => c.pending)
  return (
    <li>
      <button
        className={`thread-row ${thread.isResolved || thread.isOutdated ? 'dim' : ''}`}
        onClick={onClick}
        title={`${first?.author ?? ''}: ${first?.body ?? ''}`}
      >
        <span className="thread-where">{showPath ? `${thread.path.split('/').pop()} ` : ''}{where}</span>
        <span className="thread-author">{first?.author}</span>
        <span className="thread-snippet">{snippet(first?.body ?? '')}</span>
        {thread.comments.length > 1 && <span className="muted">+{thread.comments.length - 1}</span>}
        {pending && <span className="badge pending">Pending</span>}
        {thread.isResolved && <span className="thread-check" title="Resolved">✓</span>}
      </button>
    </li>
  )
}
