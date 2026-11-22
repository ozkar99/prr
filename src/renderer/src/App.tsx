import { useCallback, useEffect, useRef, useState } from 'react'
import { parsePRRef, type Checkout, type PRDetails, type PRRef, type PRSummary } from '../../shared/types'
import { PRView } from './PRView'
import { SetupCheck } from './SetupCheck'
import { Storage } from './Storage'

type Opened = { pr: PRDetails; checkout: Checkout }
/** Waiting for the user to OK switching their own clone to the PR. */
type PendingSwitch = { pr: PRDetails; path: string; current: string; dirty: boolean }

export function App() {
  const [queue, setQueue] = useState<PRSummary[] | undefined>()
  const [queueError, setQueueError] = useState<string>()
  const [opened, setOpened] = useState<Opened>()
  const [busy, setBusy] = useState<string>()
  const [error, setError] = useState<string>()
  const [input, setInput] = useState('')
  const [pendingSwitch, setPendingSwitch] = useState<PendingSwitch>()
  const [notice, setNotice] = useState<string>()
  const openedRef = useRef<Opened>(undefined)
  openedRef.current = opened

  const loadQueue = useCallback(() => {
    setQueueError(undefined)
    window.api.reviewQueue().then(setQueue, (e: Error) => setQueueError(e.message))
  }, [])

  // The queue loads once the setup check confirms git and a logged-in gh (see SetupCheck).
  useEffect(() => window.api.onProgress(setBusy), [])

  /** Leaves the open PR; if it switched the user's clone, switches it back. */
  const close = async () => {
    const was = openedRef.current
    setOpened(undefined)
    const local = was?.checkout.local
    if (!was || !local?.switchedFrom) return
    try {
      const restored = await window.api.restoreLocal(local.path, local.switchedFrom, was.checkout.headSha)
      setNotice(
        restored
          ? `Switched your clone back to ${local.switchedFrom}.`
          : `Left your clone at ${local.path} as it is: it changed after switching to PR #${was.pr.number} (it was on ${local.switchedFrom}).`
      )
    } catch (e) {
      setNotice(`Couldn't switch your clone back to ${local.switchedFrom}: ${(e as Error).message}`)
    }
  }

  const checkout = async (pr: PRDetails, opts: { localRepo?: string; allowSwitch?: boolean }) => {
    setPendingSwitch(undefined)
    setError(undefined)
    setBusy(`Loading ${pr.owner}/${pr.repo}#${pr.number}…`)
    try {
      const result = await window.api.checkout(pr, opts)
      if (result.kind === 'needsSwitch') {
        setPendingSwitch({ pr, path: result.path, current: result.current, dirty: result.dirty })
      } else {
        const { kind: _, ...co } = result
        setOpened({ pr, checkout: co })
      }
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(undefined)
    }
  }

  const open = async (owner: string, repo: string, number: number, localRepo?: string) => {
    // Back to the home screen, which shows loading progress.
    await close()
    setPendingSwitch(undefined)
    setNotice(undefined)
    setError(undefined)
    setBusy(`Loading ${owner}/${repo}#${number}…`)
    try {
      const pr = await window.api.getPR(owner, repo, number)
      await checkout(pr, { localRepo })
    } catch (e) {
      setError((e as Error).message)
      setBusy(undefined)
    }
  }

  // PRs from the `review-tool` command: the one we launched with, then any sent later.
  const openRef = useRef<(ref: PRRef) => void>(undefined)
  openRef.current = (ref) => void open(ref.owner, ref.repo, ref.number, ref.localRepo)
  useEffect(() => {
    void window.api.takeLaunchPR().then((ref) => ref && openRef.current?.(ref))
    return window.api.onOpenPR((ref) => openRef.current?.(ref))
  }, [])

  if (opened) {
    return <PRView key={`${opened.pr.owner}/${opened.pr.repo}#${opened.pr.number}`} {...opened} onClose={() => void close()} />
  }

  const groups: [string, PRSummary[]][] = [
    ['Review requested', queue?.filter((p) => p.reason === 'review-requested') ?? []],
    ['Your pull requests', queue?.filter((p) => p.reason === 'authored') ?? []]
  ]

  return (
    <div className="home">
      <div className="titlebar" data-tauri-drag-region />
      <div className="home-body">
        <SetupCheck onRequiredOk={loadQueue} />
        <form
          className="open-form"
          onSubmit={(e) => {
            e.preventDefault()
            const ref = parsePRRef(input)
            if (ref) void open(ref.owner, ref.repo, ref.number)
            else setError('Enter a PR URL or owner/repo#number')
          }}
        >
          <input
            autoFocus
            placeholder="Paste a PR URL or owner/repo#123"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            disabled={!!busy}
          />
          <button disabled={!!busy}>Open</button>
        </form>

        {busy && <div className="status">{busy}</div>}
        {error && <div className="error">{error}</div>}
        {notice && <div className="status">{notice}</div>}
        {pendingSwitch && (
          <div className="setup-card switch-card">
            {pendingSwitch.dirty ? (
              <div>
                Your clone at <code>{pendingSwitch.path}</code> has uncommitted changes, so it can't be switched to PR #
                {pendingSwitch.pr.number}. Commit or stash them, or review it in a separate checkout.
              </div>
            ) : (
              <div>
                Your clone at <code>{pendingSwitch.path}</code> is on <code>{pendingSwitch.current}</code>. Switch it to PR #
                {pendingSwitch.pr.number} to review it there? It switches back to <code>{pendingSwitch.current}</code> when you close
                the PR.
              </div>
            )}
            <div className="switch-actions">
              {!pendingSwitch.dirty && (
                <button className="button primary" onClick={() => void checkout(pendingSwitch.pr, { localRepo: pendingSwitch.path, allowSwitch: true })}>
                  Switch my clone
                </button>
              )}
              <button className="button" onClick={() => void checkout(pendingSwitch.pr, {})}>
                Use a separate checkout
              </button>
              {pendingSwitch.dirty && (
                <button className="button" onClick={() => void checkout(pendingSwitch.pr, { localRepo: pendingSwitch.path })}>
                  Check again
                </button>
              )}
              <button className="link" onClick={() => setPendingSwitch(undefined)}>
                Cancel
              </button>
            </div>
          </div>
        )}

        <div className="queue-header">
          <h2>Pull requests</h2>
          <button className="link" onClick={loadQueue}>
            Refresh
          </button>
        </div>
        {queueError && <div className="error">Couldn't load pull requests: {queueError}</div>}
        {!queue && !queueError && <div className="muted">Loading…</div>}
        {groups.map(([label, items]) => (
          <section key={label}>
            <h3>{label}</h3>
            {queue && items.length === 0 && <div className="muted">None</div>}
            <ul className="queue">
              {items.map((p) => (
                <li key={`${p.owner}/${p.repo}#${p.number}`}>
                  <button disabled={!!busy} onClick={() => void open(p.owner, p.repo, p.number)}>
                    <span className="repo">
                      {p.owner}/{p.repo}#{p.number}
                    </span>
                    <span className="title">
                      {p.draft && <span className="badge">Draft</span>}
                      {p.title}
                    </span>
                    <span className="muted">
                      {p.author} · {new Date(p.updatedAt).toLocaleDateString()}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ))}

        <Storage />
      </div>
    </div>
  )
}
