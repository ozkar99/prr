import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react'
import type { Checkout, EditorSelection, PRDetails } from '../../../shared/types'

export type AgentPanelHandle = {
  /** Types a prompt into the agent that points at the current selection; the user finishes the question. */
  askAboutSelection(selection: EditorSelection | null): void
  focus(): void
}

type Status = 'idle' | 'starting' | 'running' | 'exited' | { error: string }

const theme = {
  background: '#181818',
  foreground: '#cccccc',
  cursor: '#cccccc',
  selectionBackground: '#264f78',
  black: '#000000',
  red: '#f14c4c',
  green: '#23d18b',
  yellow: '#f5f543',
  blue: '#3b8eea',
  magenta: '#d670d6',
  cyan: '#29b8db',
  white: '#e5e5e5',
  brightBlack: '#666666',
  brightRed: '#f14c4c',
  brightGreen: '#23d18b',
  brightYellow: '#f5f543',
  brightBlue: '#3b8eea',
  brightMagenta: '#d670d6',
  brightCyan: '#29b8db',
  brightWhite: '#e5e5e5'
}

function selectionRef(sel: EditorSelection): string {
  const lines = sel.startLine === sel.endLine ? `line ${sel.startLine}` : `lines ${sel.startLine}-${sel.endLine}`
  const side = sel.side === 'old' ? ' (old version, before the PR)' : ''
  return `${sel.path} ${lines}${side}`
}

/**
 * The coding agent (Claude Code by default) running in a terminal, started in the
 * PR checkout. It learns what the user is looking at through the app's MCP tools
 * (get_selection etc.) or $REVIEW_TOOL_SELECTION_FILE.
 */
export const AgentPanel = forwardRef<AgentPanelHandle, { pr: PRDetails; checkout: Checkout; visible: boolean }>(
  function AgentPanel({ pr, checkout, visible }, ref) {
    const hostRef = useRef<HTMLDivElement>(null)
    const termRef = useRef<Terminal>(undefined)
    const fitRef = useRef<FitAddon>(undefined)
    const idRef = useRef<number>(undefined)
    const pendingInput = useRef<string>(undefined)
    const lastOutputAt = useRef(0)
    const [status, setStatus] = useState<Status>('idle')

    const start = useCallback(async () => {
      const host = hostRef.current
      if (!host) return
      termRef.current?.dispose()
      host.replaceChildren()
      const term = new Terminal({
        fontFamily: 'ui-monospace, Menlo, monospace',
        fontSize: 12,
        lineHeight: 1.15,
        cursorBlink: true,
        theme,
        allowProposedApi: true,
        macOptionIsMeta: true,
        scrollback: 5000
      })
      const fit = new FitAddon()
      term.loadAddon(fit)
      term.loadAddon(new WebLinksAddon((_e, uri) => void window.api.openExternal(uri)))
      term.open(host)
      fit.fit()
      termRef.current = term
      fitRef.current = fit
      setStatus('starting')
      try {
        const id = await window.api.agentStart(
          {
            root: checkout.root,
            prLabel: `${pr.owner}/${pr.repo}#${pr.number} ("${pr.title}")`,
            mergeBase: checkout.mergeBase,
            cols: term.cols,
            rows: term.rows
          },
          (bytes) => {
            lastOutputAt.current = performance.now()
            term.write(bytes)
          }
        )
        idRef.current = id
        term.onData((data) => void window.api.ptyWrite(id, data).catch(() => {}))
        setStatus('running')
        if (pendingInput.current) {
          // Type into the agent's prompt once it has finished drawing its UI
          // (output has gone quiet), or after 10s at the latest.
          const text = pendingInput.current
          pendingInput.current = undefined
          const startedAt = performance.now()
          const wait = setInterval(() => {
            const now = performance.now()
            const settled = lastOutputAt.current > 0 && now - lastOutputAt.current > 700 && now - startedAt > 1500
            if (settled || now - startedAt > 10000) {
              clearInterval(wait)
              if (idRef.current === id) void window.api.ptyWrite(id, text)
            }
          }, 100)
        }
        term.focus()
      } catch (e) {
        setStatus({ error: (e as Error).message })
      }
    }, [pr, checkout])

    // Start on first open; keep running while hidden.
    useEffect(() => {
      if (visible && status === 'idle') void start()
      if (visible) {
        requestAnimationFrame(() => {
          fitRef.current?.fit()
          termRef.current?.focus()
        })
      }
    }, [visible, status, start])

    useEffect(() => {
      const off = window.api.onPtyExit((id) => {
        if (id === idRef.current) {
          idRef.current = undefined
          setStatus('exited')
        }
      })
      return () => {
        off()
        if (idRef.current !== undefined) void window.api.ptyKill(idRef.current)
        termRef.current?.dispose()
      }
    }, [])

    // Keep the terminal sized to the panel.
    useEffect(() => {
      const host = hostRef.current
      if (!host) return
      const observer = new ResizeObserver(() => {
        const term = termRef.current
        if (!term || !host.offsetHeight) return
        fitRef.current?.fit()
        if (idRef.current !== undefined) void window.api.ptyResize(idRef.current, term.cols, term.rows).catch(() => {})
      })
      observer.observe(host)
      return () => observer.disconnect()
    }, [])

    useImperativeHandle(
      ref,
      () => ({
        askAboutSelection(selection) {
          const text = selection ? `About ${selectionRef(selection)}: ` : ''
          if (idRef.current === undefined) {
            pendingInput.current = text
            if (status !== 'starting') void start()
            return
          }
          if (text) void window.api.ptyWrite(idRef.current, text)
          termRef.current?.focus()
        },
        focus() {
          termRef.current?.focus()
        }
      }),
      [start, status]
    )

    return (
      <div className="agent-panel">
        <div className="agent-terminal" ref={hostRef} />
        {status === 'exited' && (
          <div className="agent-overlay">
            The agent exited.{' '}
            <button className="button primary" onClick={() => void start()}>
              Restart
            </button>
          </div>
        )}
        {typeof status === 'object' && (
          <div className="agent-overlay error">
            {status.error}{' '}
            <button className="button" onClick={() => void start()}>
              Retry
            </button>
          </div>
        )}
      </div>
    )
  }
)
