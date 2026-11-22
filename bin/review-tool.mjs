#!/usr/bin/env node
// review-tool: open a GitHub pull request in the review app.
//
//   review-tool https://github.com/owner/repo/pull/123
//   review-tool owner/repo#123
//   review-tool 123          (inside a clone: PR #123 of that repo)
//   review-tool              (inside a clone: the PR for the current branch, else just open the app)
//
// If the app is already running, the PR opens in the existing window.

import { execFileSync, spawn } from 'node:child_process'
import { existsSync, realpathSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const appDir = join(dirname(realpathSync(fileURLToPath(import.meta.url))), '..')

function usage(code = 0) {
  console.log(`Usage: review-tool [PR]

PR can be:
  https://github.com/owner/repo/pull/123   a PR URL
  owner/repo#123                           a PR reference
  123                                      PR number in the GitHub repo of the current directory
  (nothing)                                the PR for the current branch, or just open the app`)
  process.exit(code)
}

function fail(message) {
  console.error(`review-tool: ${message}`)
  process.exit(1)
}

/** Runs gh in the current directory; returns trimmed stdout or undefined on failure. */
function gh(args) {
  try {
    return execFileSync('gh', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim()
  } catch {
    return undefined
  }
}

function resolvePR(arg) {
  if (arg === undefined) return gh(['pr', 'view', '--json', 'url', '--jq', '.url']) // current branch, if it has a PR
  if (/github\.com\/[^/]+\/[^/]+\/pull\/\d+/.test(arg)) return arg
  const ref = arg.match(/^([\w.-]+)\/([\w.-]+)#(\d+)$/)
  if (ref) return `https://github.com/${ref[1]}/${ref[2]}/pull/${ref[3]}`
  const num = arg.match(/^#?(\d+)$/)
  if (num) {
    const repo = gh(['repo', 'view', '--json', 'nameWithOwner', '--jq', '.nameWithOwner'])
    if (!repo) fail(`"${arg}" is a PR number, but the current directory isn't a GitHub repository clone`)
    return `https://github.com/${repo}/pull/${num[1]}`
  }
  fail(`don't understand "${arg}" (expected a PR URL, owner/repo#123, or a number)`)
}

const args = process.argv.slice(2)
if (args.includes('-h') || args.includes('--help')) usage()
if (args.length > 1) usage(1)

if (!existsSync(join(appDir, 'out', 'main', 'index.js'))) {
  fail(`the app isn't built yet. Run:\n  cd ${appDir} && npm run build`)
}

const url = resolvePR(args[0])
const electron = createRequire(join(appDir, 'package.json'))('electron') // path to the Electron binary
const appArgs = [appDir, ...(url ? [`--open-pr=${url}`] : [])]

console.log(url ? `Opening ${url}` : 'Opening review-tool')
spawn(electron, appArgs, { detached: true, stdio: 'ignore' }).unref()
