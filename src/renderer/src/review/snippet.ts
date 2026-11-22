/** First meaningful line of a Markdown/HTML comment as plain text, for one-line previews. */
export function snippet(markdown: string): string {
  const text = markdown
    .replace(/<!--[\s\S]*?-->/g, '') // HTML comments (bots hide metadata here)
    .replace(/```[\s\S]*?```/g, ' ') // fenced code blocks
    .replace(/<[^>]+>/g, ' ') // HTML tags
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '') // images
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1') // links -> their text
  for (const raw of text.split('\n')) {
    const line = raw
      .replace(/^\s*(?:>\s*)+/, '') // blockquotes
      .replace(/^\s*#{1,6}\s+/, '') // headings
      .replace(/^\s*(?:[-*+]|\d+\.)\s+(?:\[[ xX]\]\s+)?/, '') // list items / task boxes
      .replace(/(\*\*|__|\*|_|~~|`)(.+?)\1/g, '$2') // emphasis and inline code
      .trim()
    if (line && !/^[-*_=\s|:]+$/.test(line)) return line // skip rules and table separators
  }
  return ''
}
