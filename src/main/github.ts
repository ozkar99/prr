import { Octokit } from '@octokit/rest'
import type { CommentableRanges, FileStatus, NewThread, PRDetails, PRSummary, ReviewEvent, ReviewState, ReviewThread } from '../shared/types'
import { run } from './exec'

let client: Octokit | undefined

/** Reuses the token from the GitHub CLI so there is no separate login. */
async function gh(): Promise<Octokit> {
  if (!client) {
    const token = (await run('gh', ['auth', 'token'])).trim()
    client = new Octokit({ auth: token, userAgent: 'review-tool' })
  }
  return client
}

export async function reviewQueue(): Promise<PRSummary[]> {
  const octokit = await gh()
  const queries: [PRSummary['reason'], string][] = [
    ['review-requested', 'is:pr is:open archived:false review-requested:@me'],
    ['authored', 'is:pr is:open archived:false author:@me']
  ]
  const results = await Promise.all(
    queries.map(async ([reason, q]) => {
      const { data } = await octokit.search.issuesAndPullRequests({ q, sort: 'updated', per_page: 50 })
      return data.items.map((item): PRSummary => {
        const [owner, repo] = item.repository_url.split('/').slice(-2)
        return {
          owner,
          repo,
          number: item.number,
          title: item.title,
          author: item.user?.login ?? '',
          updatedAt: item.updated_at,
          draft: item.draft ?? false,
          reason
        }
      })
    })
  )
  return results.flat()
}

export async function getPR(owner: string, repo: string, number: number): Promise<PRDetails> {
  const octokit = await gh()
  const { data: pr } = await octokit.pulls.get({ owner, repo, pull_number: number })
  const files = await octokit.paginate(octokit.pulls.listFiles, { owner, repo, pull_number: number, per_page: 100 })
  return {
    owner,
    repo,
    number,
    title: pr.title,
    body: pr.body ?? '',
    author: pr.user.login,
    url: pr.html_url,
    baseRef: pr.base.ref,
    baseSha: pr.base.sha,
    headSha: pr.head.sha,
    files: files.map((f) => ({
      path: f.filename,
      previousPath: f.previous_filename,
      status: f.status as FileStatus,
      additions: f.additions,
      deletions: f.deletions,
      commentable: f.patch ? commentableRanges(f.patch) : undefined
    }))
  }
}

/** Line ranges per side from the unified diff's hunk headers (`@@ -a,b +c,d @@`). */
export function commentableRanges(patch: string): CommentableRanges {
  const ranges: CommentableRanges = { LEFT: [], RIGHT: [] }
  for (const m of patch.matchAll(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/gm)) {
    const [left, leftLen, right, rightLen] = [Number(m[1]), Number(m[2] ?? 1), Number(m[3]), Number(m[4] ?? 1)]
    if (leftLen > 0) ranges.LEFT.push([left, left + leftLen - 1])
    if (rightLen > 0) ranges.RIGHT.push([right, right + rightLen - 1])
  }
  return ranges
}

// ---- Reviews (GraphQL: the only API with pending reviews and threads) ----

const threadFields = `
  id path diffSide line startLine isResolved isOutdated viewerCanResolve viewerCanUnresolve
  comments(first: 100) { nodes { id body createdAt url state viewerCanDelete author { login avatarUrl } } }`

type ThreadNode = {
  id: string
  path: string
  diffSide: 'LEFT' | 'RIGHT'
  line: number | null
  startLine: number | null
  isResolved: boolean
  isOutdated: boolean
  viewerCanResolve: boolean
  viewerCanUnresolve: boolean
  comments: {
    nodes: {
      id: string
      body: string
      createdAt: string
      url: string
      state: string
      viewerCanDelete: boolean
      author: { login: string; avatarUrl: string } | null
    }[]
  }
}

const toThread = (t: ThreadNode): ReviewThread => ({
  id: t.id,
  path: t.path,
  side: t.diffSide,
  line: t.line,
  startLine: t.startLine,
  isResolved: t.isResolved,
  isOutdated: t.isOutdated,
  viewerCanResolve: t.viewerCanResolve,
  viewerCanUnresolve: t.viewerCanUnresolve,
  comments: t.comments.nodes.map((c) => ({
    id: c.id,
    author: c.author?.login ?? 'ghost',
    authorAvatar: c.author?.avatarUrl ?? '',
    body: c.body,
    createdAt: c.createdAt,
    url: c.url,
    pending: c.state === 'PENDING',
    viewerCanDelete: c.viewerCanDelete
  }))
})

export async function reviewState(owner: string, repo: string, number: number): Promise<ReviewState> {
  const octokit = await gh()
  const threads: ReviewThread[] = []
  let after: string | null = null
  let base: Omit<ReviewState, 'threads'> | undefined
  do {
    const res: {
      viewer: { login: string }
      repository: {
        pullRequest: {
          id: string
          reviews: { nodes: { id: string; viewerDidAuthor: boolean }[] }
          reviewThreads: { pageInfo: { hasNextPage: boolean; endCursor: string | null }; nodes: ThreadNode[] }
        }
      }
    } = await octokit.graphql(
      `query($owner: String!, $repo: String!, $number: Int!, $after: String) {
        viewer { login }
        repository(owner: $owner, name: $repo) {
          pullRequest(number: $number) {
            id
            reviews(states: PENDING, first: 20) { nodes { id viewerDidAuthor } }
            reviewThreads(first: 100, after: $after) {
              pageInfo { hasNextPage endCursor }
              nodes { ${threadFields} }
            }
          }
        }
      }`,
      { owner, repo, number, after }
    )
    const pr = res.repository.pullRequest
    base ??= {
      prNodeId: pr.id,
      viewer: res.viewer.login,
      pendingReviewId: pr.reviews.nodes.find((r) => r.viewerDidAuthor)?.id
    }
    threads.push(...pr.reviewThreads.nodes.map(toThread))
    after = pr.reviewThreads.pageInfo.hasNextPage ? pr.reviewThreads.pageInfo.endCursor : null
  } while (after)
  // GitHub is the source of truth for whether a pending review exists.
  if (base!.pendingReviewId) pendingReviews.set(base!.prNodeId, Promise.resolve(base!.pendingReviewId))
  else pendingReviews.delete(base!.prNodeId)
  return { ...base!, threads }
}

// One pending review per PR; concurrent comment adds must not each create one.
const pendingReviews = new Map<string, Promise<string>>()

function ensurePendingReview(state: ReviewState, headSha: string): Promise<string> {
  let review = pendingReviews.get(state.prNodeId)
  if (!review) {
    review = state.pendingReviewId
      ? Promise.resolve(state.pendingReviewId)
      : gh()
          .then((octokit) =>
            octokit.graphql<{ addPullRequestReview: { pullRequestReview: { id: string } } }>(
              `mutation($pr: ID!, $sha: GitObjectID!) {
                addPullRequestReview(input: { pullRequestId: $pr, commitOID: $sha }) { pullRequestReview { id } }
              }`,
              { pr: state.prNodeId, sha: headSha }
            )
          )
          .then((r) => r.addPullRequestReview.pullRequestReview.id)
    review.catch(() => pendingReviews.delete(state.prNodeId))
    pendingReviews.set(state.prNodeId, review)
  }
  return review
}

export async function addThread(state: ReviewState, headSha: string, t: NewThread): Promise<void> {
  const reviewId = await ensurePendingReview(state, headSha)
  const multiLine = t.startLine !== undefined && t.startLine < t.line
  await (await gh()).graphql(
    `mutation($review: ID!, $path: String!, $body: String!, $line: Int!, $side: DiffSide!, $startLine: Int, $startSide: DiffSide) {
      addPullRequestReviewThread(input: {
        pullRequestReviewId: $review, path: $path, body: $body, line: $line, side: $side,
        startLine: $startLine, startSide: $startSide, subjectType: LINE
      }) { thread { id } }
    }`,
    {
      review: reviewId,
      path: t.path,
      body: t.body,
      line: t.line,
      side: t.side,
      startLine: multiLine ? t.startLine : null,
      startSide: multiLine ? t.side : null
    }
  )
}

export async function reply(state: ReviewState, headSha: string, threadId: string, body: string): Promise<void> {
  const reviewId = await ensurePendingReview(state, headSha)
  await (await gh()).graphql(
    `mutation($review: ID!, $thread: ID!, $body: String!) {
      addPullRequestReviewThreadReply(input: { pullRequestReviewId: $review, pullRequestReviewThreadId: $thread, body: $body }) { comment { id } }
    }`,
    { review: reviewId, thread: threadId, body }
  )
}

export async function deleteComment(id: string): Promise<void> {
  await (await gh()).graphql(`mutation($id: ID!) { deletePullRequestReviewComment(input: { id: $id }) { clientMutationId } }`, { id })
}

export async function setResolved(threadId: string, resolved: boolean): Promise<void> {
  const mutation = resolved ? 'resolveReviewThread' : 'unresolveReviewThread'
  await (await gh()).graphql(`mutation($id: ID!) { ${mutation}(input: { threadId: $id }) { thread { id } } }`, { id: threadId })
}

export async function submitReview(state: ReviewState, headSha: string, event: ReviewEvent, body: string): Promise<void> {
  const octokit = await gh()
  const pendingId = state.pendingReviewId ?? (await pendingReviews.get(state.prNodeId)?.catch(() => undefined))
  if (pendingId) {
    await octokit.graphql(
      `mutation($review: ID!, $event: PullRequestReviewEvent!, $body: String) {
        submitPullRequestReview(input: { pullRequestReviewId: $review, event: $event, body: $body }) { pullRequestReview { id } }
      }`,
      { review: pendingId, event, body: body || null }
    )
  } else {
    // No inline comments: create and submit in one step.
    await octokit.graphql(
      `mutation($pr: ID!, $sha: GitObjectID!, $event: PullRequestReviewEvent!, $body: String) {
        addPullRequestReview(input: { pullRequestId: $pr, commitOID: $sha, event: $event, body: $body }) { pullRequestReview { id } }
      }`,
      { pr: state.prNodeId, sha: headSha, event, body: body || null }
    )
  }
  pendingReviews.delete(state.prNodeId)
}
