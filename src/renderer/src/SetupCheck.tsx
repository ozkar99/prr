import { useCallback, useEffect, useState } from 'react'
import type { ToolStatus } from '../../shared/types'

/**
 * First-run setup check on the home screen: required tools that are missing
 * (git, logged-in gh) are shown up front; optional ones (language servers,
 * the agent) behind a toggle, each with the command that installs it.
 */
export function SetupCheck({ onRequiredOk }: { onRequiredOk: () => void }) {
  const [tools, setTools] = useState<ToolStatus[]>()
  const [showOptional, setShowOptional] = useState(false)
  const [checking, setChecking] = useState(false)

  const check = useCallback(async () => {
    setChecking(true)
    try {
      const result = await window.api.checkTools()
      setTools(result)
      if (result.every((t) => !t.required || t.ok)) onRequiredOk()
    } finally {
      setChecking(false)
    }
  }, [onRequiredOk])

  useEffect(() => {
    void check()
  }, [check])

  if (!tools) return null
  const required = tools.filter((t) => t.required && !t.ok)
  const optional = tools.filter((t) => !t.required)
  const optionalMissing = optional.filter((t) => !t.ok)

  return (
    <div className="setup">
      {required.length > 0 && (
        <div className="setup-card required">
          <div className="setup-title">Review Tool needs a couple of things first</div>
          <ToolList tools={required} />
          <button className="button primary" onClick={() => void check()} disabled={checking}>
            {checking ? 'Checking…' : 'Check again'}
          </button>
        </div>
      )}
      {required.length === 0 && (
        <div className="setup-summary">
          <span className="muted">
            {optionalMissing.length === 0
              ? 'All tools found.'
              : `${optionalMissing.length} optional tool${optionalMissing.length === 1 ? '' : 's'} not installed: ${optionalMissing.map((t) => t.name).join(', ')}.`}
          </span>
          <button className="link" onClick={() => setShowOptional(!showOptional)}>
            {showOptional ? 'Hide' : 'Details'}
          </button>
          <button className="link" onClick={() => void check()} disabled={checking}>
            {checking ? 'Checking…' : 'Check again'}
          </button>
        </div>
      )}
      {required.length === 0 && showOptional && (
        <div className="setup-card">
          <ToolList tools={optional} />
        </div>
      )}
    </div>
  )
}

function ToolList({ tools }: { tools: ToolStatus[] }) {
  return (
    <ul className="tool-list">
      {tools.map((t) => (
        <li key={t.name}>
          <span className={t.ok ? 'add' : t.required ? 'del' : 'muted'}>{t.ok ? '✓' : '✗'}</span>
          <strong>{t.name}</strong>
          <span className="muted">{t.purpose}</span>
          {!t.ok && <span className="tool-problem">{t.problem}</span>}
          {t.fix && (
            <code className="tool-fix" title="Click to copy" onClick={() => void navigator.clipboard?.writeText(t.fix!)}>
              {t.fix}
            </code>
          )}
        </li>
      ))}
    </ul>
  )
}
