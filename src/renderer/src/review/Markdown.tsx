import { StandaloneServices } from '@codingame/monaco-vscode-api'
import { IMarkdownRendererService } from '@codingame/monaco-vscode-api/vscode/vs/platform/markdown/browser/markdownRenderer.service'
import { useEffect, useRef } from 'react'

/**
 * Renders GitHub-flavoured Markdown with VS Code's own renderer (the one used
 * for hovers): fenced code blocks are colored with the editor's grammars and
 * theme, and HTML goes through VS Code's sanitizer. Content is untrusted, so
 * command links are disabled.
 */
export function Markdown({ text, className }: { text: string; className?: string }) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const host = ref.current
    if (!host) return
    const rendered = StandaloneServices.get(IMarkdownRendererService).render({
      value: text,
      isTrusted: false,
      supportHtml: true,
      supportThemeIcons: false
    })
    host.replaceChildren(rendered.element)
    return () => rendered.dispose()
  }, [text])

  return <div ref={ref} className={`markdown ${className ?? ''}`} />
}
