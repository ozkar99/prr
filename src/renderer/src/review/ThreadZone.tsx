import { useEffect, useRef, useState } from 'react'
import type { DiffSide, ReviewComment, ReviewThread } from '../../../shared/types'
import { threadsAt, useReview, type ReviewStore } from './ReviewStore'

export type ComposerRequest = { startLine?: number }

type Props = {
  store: ReviewStore
  path: string
  side: DiffSide
  line: number
  composer?: ComposerRequest
  onCloseComposer: () => void
  /** Receives the content element so the host can size the view zone to it. */
  measureRef?: (el: HTMLDivElement | null) => void
}

/** Everything shown under one diff line: its threads, plus a new-comment box if open. */
export function ThreadZone({ store, path, side, line, composer, onCloseComposer, measureRef }: Props) {
  const { state } = useReview(store)
  const threads = threadsAt(state, path, side, line)
  return (
    <div className="thread-zone-inner" ref={measureRef}>
      {threads.map((t) => (
        <Thread key={t.id} store={store} thread={t} />
      ))}
      {composer && (
        <div className="thread">
          <div className="thread-header">
            New comment on {composer.startLine ? `lines ${composer.startLine}–${line}` : `line ${line}`}
            {side === 'LEFT' && <span className="muted"> (old version)</span>}
          </div>
          <Composer
            autoFocus
            submitLabel={state?.pendingReviewId ? 'Add review comment' : 'Start a review'}
            onCancel={onCloseComposer}
            onSubmit={async (body) => {
              await store.addThread(path, side, line, composer.startLine, body)
              onCloseComposer()
            }}
          />
        </div>
      )}
    </div>
  )
}

export function Thread({ store, thread }: { store: ReviewStore; thread: ReviewThread }) {
  const [collapsed, setCollapsed] = useState(thread.isResolved)
  const [replying, setReplying] = useState(false)
  const [error, setError] = useState<string>()
  const range = thread.isOutdated
    ? 'Outdated'
    : thread.startLine && thread.startLine !== thread.line
      ? `Lines ${thread.startLine}–${thread.line}`
      : `Line ${thread.line}`

  const toggleResolved = async () => {
    setError(undefined)
    try {
      await store.setResolved(thread.id, !thread.isResolved)
      setCollapsed(!thread.isResolved)
    } catch (e) {
      setError((e as Error).message)
    }
  }

  return (
    <div className={`thread ${thread.isResolved ? 'resolved' : ''}`}>
      <div className="thread-header">
        <button className="link" onClick={() => setCollapsed(!collapsed)}>
          {collapsed ? '▸' : '▾'}
        </button>
        <span>{range}</span>
        {thread.isResolved && <span className="badge">Resolved</span>}
        {collapsed && (
          <span className="muted thread-summary">
            {thread.comments[0]?.author}: {thread.comments[0]?.body.split('\n')[0]}
            {thread.comments.length > 1 && ` (+${thread.comments.length - 1})`}
          </span>
        )}
        <span className="spacer" />
        {(thread.isResolved ? thread.viewerCanUnresolve : thread.viewerCanResolve) && (
          <button className="link" onClick={() => void toggleResolved()}>
            {thread.isResolved ? 'Unresolve' : 'Resolve'}
          </button>
        )}
      </div>
      {!collapsed && (
        <>
          {thread.comments.map((c) => (
            <Comment key={c.id} store={store} comment={c} />
          ))}
          {error && <div className="error">{error}</div>}
          {replying ? (
            <Composer
              autoFocus
              submitLabel="Reply"
              onCancel={() => setReplying(false)}
              onSubmit={async (body) => {
                await store.reply(thread.id, body)
                setReplying(false)
              }}
            />
          ) : (
            <button className="reply-placeholder" onClick={() => setReplying(true)}>
              Reply…
            </button>
          )}
        </>
      )}
    </div>
  )
}

function Comment({ store, comment }: { store: ReviewStore; comment: ReviewComment }) {
  const [deleting, setDeleting] = useState(false)
  return (
    <div className="comment">
      <div className="comment-header">
        {comment.authorAvatar && <img className="avatar" src={comment.authorAvatar} alt="" />}
        <strong>{comment.author}</strong>
        <span className="muted">{new Date(comment.createdAt).toLocaleString()}</span>
        {comment.pending && <span className="badge pending">Pending</span>}
        <span className="spacer" />
        {comment.pending && comment.viewerCanDelete && (
          <button
            className="link"
            disabled={deleting}
            onClick={async () => {
              setDeleting(true)
              await store.deleteComment(comment.id).catch(() => setDeleting(false))
            }}
          >
            Delete
          </button>
        )}
      </div>
      <div className="comment-body">{comment.body}</div>
    </div>
  )
}

export function Composer({
  autoFocus,
  submitLabel,
  onSubmit,
  onCancel,
  allowEmpty
}: {
  autoFocus?: boolean
  submitLabel: string
  onSubmit: (body: string) => Promise<void>
  onCancel?: () => void
  allowEmpty?: boolean
}) {
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string>()
  const ref = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (autoFocus) ref.current?.focus()
  }, [autoFocus])

  const submit = async () => {
    if ((!body.trim() && !allowEmpty) || busy) return
    setBusy(true)
    setError(undefined)
    try {
      await onSubmit(body.trim())
      setBody('')
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="composer">
      <textarea
        ref={ref}
        value={body}
        rows={3}
        placeholder="Leave a comment (Markdown supported). ⌘↩ to submit, Esc to cancel"
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault()
            void submit()
          } else if (e.key === 'Escape' && onCancel) {
            e.preventDefault()
            onCancel()
          }
        }}
        disabled={busy}
      />
      {error && <div className="error">{error}</div>}
      <div className="composer-actions">
        {onCancel && (
          <button className="button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
        )}
        <button className="button primary" onClick={() => void submit()} disabled={busy || (!body.trim() && !allowEmpty)}>
          {busy ? 'Saving…' : submitLabel}
        </button>
      </div>
    </div>
  )
}
