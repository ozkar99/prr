import { useEffect, useRef, useState } from 'react'
import type { ReviewEvent, ReviewState } from '../../../shared/types'
import { pendingCount, type ReviewStore } from './ReviewStore'
import { Composer } from './ThreadZone'

const events: { value: ReviewEvent; label: string; help: string }[] = [
  { value: 'COMMENT', label: 'Comment', help: 'Submit general feedback without explicit approval.' },
  { value: 'APPROVE', label: 'Approve', help: 'Submit feedback and approve merging these changes.' },
  { value: 'REQUEST_CHANGES', label: 'Request changes', help: 'Submit feedback that must be addressed before merging.' }
]

/** "Finish your review" popover: summary, event choice, submit. */
export function ReviewPanel({ store, state, prAuthor, onClose }: { store: ReviewStore; state: ReviewState; prAuthor: string; onClose: () => void }) {
  const isOwnPR = state.viewer === prAuthor
  const [event, setEvent] = useState<ReviewEvent>('COMMENT')
  const [done, setDone] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const pending = pendingCount(state)

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node) && !(e.target as HTMLElement).closest('.review-button-wrap')) onClose()
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [onClose])

  if (done) {
    return (
      <div className="review-panel" ref={ref}>
        <div className="review-panel-title">Review submitted</div>
        <button className="button" onClick={onClose}>
          Close
        </button>
      </div>
    )
  }

  return (
    <div className="review-panel" ref={ref}>
      <div className="review-panel-title">Finish your review</div>
      <div className="muted">{pending > 0 ? `${pending} pending comment${pending === 1 ? '' : 's'} will be published.` : 'No pending comments.'}</div>
      <Composer
        autoFocus
        allowEmpty={event === 'APPROVE' || pending > 0}
        submitLabel="Submit review"
        onCancel={onClose}
        onSubmit={async (body) => {
          if (event === 'REQUEST_CHANGES' && !body && pending === 0) throw new Error('Requesting changes needs a comment.')
          await store.submit(event, body)
          setDone(true)
        }}
      />
      <div className="review-events">
        {events.map((e) => {
          const disabled = isOwnPR && e.value !== 'COMMENT'
          return (
            <label key={e.value} className={disabled ? 'disabled' : ''} title={disabled ? "You can't approve or request changes on your own pull request." : e.help}>
              <input type="radio" name="review-event" checked={event === e.value} disabled={disabled} onChange={() => setEvent(e.value)} />
              <span>
                <strong>{e.label}</strong>
                <span className="muted"> — {e.help}</span>
              </span>
            </label>
          )
        })}
      </div>
    </div>
  )
}
