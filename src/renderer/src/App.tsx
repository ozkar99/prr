import { useCallback, useEffect, useState } from 'react'
import type { Checkout, PRDetails, PRSummary } from '../../shared/types'
import { PRView } from './PRView'

type Opened = { pr: PRDetails; checkout: Checkout }

function parsePRRef(input: string): { owner: string; repo: string; number: number } | undefined {
  const s = input.trim()
  const url = s.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/)
  const short = s.match(/^([\w.-]+)\/([\w.-]+)#(\d+)$/)
  const m = url ?? short
  return m ? { owner: m[1], repo: m[2], number: Number(m[3]) } : undefined
}

export function App() {
  const [queue, setQueue] = useState<PRSummary[] | undefined>()
  const [queueError, setQueueError] = useState<string>()
  const [opened, setOpened] = useState<Opened>()
  const [busy, setBusy] = useState<string>()
  const [error, setError] = useState<string>()
  const [input, setInput] = useState('')

  const loadQueue = useCallback(() => {
    setQueueError(undefined)
    window.api.reviewQueue().then(setQueue, (e: Error) => setQueueError(e.message))
  }, [])

  useEffect(loadQueue, [loadQueue])
  useEffect(() => window.api.onProgress(setBusy), [])

  const open = async (owner: string, repo: string, number: number) => {
    setError(undefined)
    setBusy(`Loading ${owner}/${repo}#${number}…`)
    try {
      const pr = await window.api.getPR(owner, repo, number)
      const checkout = await window.api.checkout(pr)
      setOpened({ pr, checkout })
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(undefined)
    }
  }

  if (opened) {
    return <PRView key={`${opened.pr.owner}/${opened.pr.repo}#${opened.pr.number}`} {...opened} onClose={() => setOpened(undefined)} />
  }

  const groups: [string, PRSummary[]][] = [
    ['Review requested', queue?.filter((p) => p.reason === 'review-requested') ?? []],
    ['Your pull requests', queue?.filter((p) => p.reason === 'authored') ?? []]
  ]

  return (
    <div className="home">
      <div className="titlebar" />
      <div className="home-body">
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
      </div>
    </div>
  )
}
