/**
 * Markdown to plain text formatter for WeChat replies.
 *
 * Converts common Markdown constructs to readable plain text that
 * WeChat can display. Core principle: information must not be lost.
 */

/**
 * Strip Markdown formatting from text, converting to a plain-text
 * representation suitable for WeChat.
 *
 * Transformations:
 *   - Bold (**text**)  → text
 *   - Italic (*text*)  → text
 *   - Inline code (`code`) → code
 *   - Code blocks (```lang\n...\n```) → \n...\n (content preserved)
 *   - Headings (# text) → 【text】
 *   - Lists (- / * item) → · item
 *   - Links [text](url) → text (url)
 *   - Horizontal rules (---) → removed
 */
export function stripMarkdown(text: string): string {
  if (!text) return '';

  let result = text;

  // 1. Strip code blocks first (with or without language identifier)
  result = result.replace(/```[\w]*\n?([\s\S]*?)\n?```\n?/g, '\n$1\n');

  // 2. Remove horizontal rules (---, ***, ___ on their own line)
  result = result.replace(/^([-*_])\1{2,}$/gm, '');

  // 3. Convert headings: # text → 【text】
  result = result.replace(/^#{1,6}\s+(.+)$/gm, (_match, content: string) => {
    return `【${content}】`;
  });

  // 4. Convert list items: - or * → ·
  result = result.replace(/^([ \t]*)[-*]\s+/gm, '$1· ');

  // 5. Convert links: [text](url) → text (url)
  result = result.replace(/\[([^\]]*)\]\(([^)]*)\)/g, (_match, text: string, url: string) => {
    if (!text) return url;
    return `${text} (${url})`;
  });

  // 6. Strip inline code: `code` → code
  result = result.replace(/`([^`]+)`/g, '$1');

  // 7. Strip bold + italic (***text*** → text)
  result = result.replace(/\*\*\*(.+?)\*\*\*/g, '$1');

  // 8. Strip bold (**text** → text)
  result = result.replace(/\*\*(.+?)\*\*/g, '$1');

  // 9. Strip italic (*text* → text)
  result = result.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '$1');

  return result;
}
