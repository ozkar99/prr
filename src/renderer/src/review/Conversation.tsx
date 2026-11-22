import type { ConversationItem, PRDetails } from '../../../shared/types'
import { Markdown } from './Markdown'
import { useReview, type ReviewStore } from './ReviewStore'
import { Composer } from './ThreadZone'

const reviewLabel: Record<NonNullable<ConversationItem['reviewState']>, string> = {
  APPROVED: 'approved these changes',
  CHANGES_REQUESTED: 'requested changes',
  COMMENTED: 'reviewed',
  DISMISSED: 'reviewed (dismissed)'
}

/** PR description plus everything not attached to a file: general comments and review summaries. */
export function Conversation({ pr, store }: { pr: PRDetails; store: ReviewStore }) {
  const { state, loading, error } = useReview(store)
  return (
    <div className="conversation">
      <div className="conversation-inner">
        <h1 className="conversation-title">
          {pr.title} <span className="muted">#{pr.number}</span>
        </h1>
        <div className="muted conversation-meta">
          {pr.author} wants to merge into <code>{pr.baseRef}</code>
        </div>

        <article className="conv-item">
          <div className="comment-header">
            <strong>{pr.author}</strong>
            <span className="muted">description</span>
          </div>
          {pr.body.trim() ? <Markdown className="comment-body" text={pr.body} /> : <div className="muted">No description provided.</div>}
        </article>

        {error && <div className="error">Couldn't load the conversation: {error}</div>}
        {loading && !state && <div className="muted">Loading…</div>}

        {state?.conversation.map((item) => (
          <article key={item.id} className={`conv-item ${item.reviewState ? `review-${item.reviewState.toLowerCase()}` : ''}`} id={`conv-${item.id}`}>
            <div className="comment-header">
              {item.authorAvatar && <img className="avatar" src={item.authorAvatar} alt="" />}
              <strong>{item.author}</strong>
              {item.reviewState && <span className="review-verdict">{reviewLabel[item.reviewState]}</span>}
              <span className="muted">{new Date(item.createdAt).toLocaleString()}</span>
            </div>
            {item.body.trim() && <Markdown className="comment-body" text={item.body} />}
          </article>
        ))}

        {state && (
          <div className="conv-item">
            <div className="comment-header">
              <strong>Add a comment</strong>
              <span className="muted">posts immediately, separate from your review</span>
            </div>
            <Composer submitLabel="Comment" onSubmit={(body) => store.addPRComment(body)} />
          </div>
        )}
      </div>
    </div>
  )
}
