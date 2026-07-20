/**
 * /resume command handler for listing and switching Claude Code sessions
 * within the current workspace.
 *
 * Uses the SDK's `listSessions({ dir })` API to enumerate sessions stored
 * under `~/.claude/projects/<encoded-cwd>/`.
 *
 * Commands:
 * - `/resume`          - list all sessions in the current workspace
 * - `/resume <id>`     - switch to a session by full UUID or unique prefix
 */

import { listSessions, type SDKSessionInfo } from '@anthropic-ai/claude-agent-sdk';

/**
 * List all Claude Code sessions for a given workspace directory,
 * sorted by last-modified time (most recent first).
 */
export async function listSessionsForCwd(cwd: string): Promise<SDKSessionInfo[]> {
  const sessions = await listSessions({ dir: cwd });
  return [...sessions].sort((a, b) => b.lastModified - a.lastModified);
}

/**
 * Resolve user input (full UUID or unique prefix) to a single session.
 *
 * Returns:
 * - `{ session }` on unique match
 * - `{ error, matches }` on ambiguous prefix
 * - `{ error }` on no match
 */
export function resolveSession(
  input: string,
  sessions: SDKSessionInfo[],
): { session?: SDKSessionInfo; error?: string; matches?: SDKSessionInfo[] } {
  const trimmed = input.trim().toLowerCase();
  if (!trimmed) {
    return { error: 'session ID 不能为空' };
  }

  // Exact match (case-insensitive)
  const exact = sessions.find((s) => s.sessionId.toLowerCase() === trimmed);
  if (exact) {
    return { session: exact };
  }

  // Prefix match
  const prefixMatches = sessions.filter((s) =>
    s.sessionId.toLowerCase().startsWith(trimmed),
  );

  if (prefixMatches.length === 0) {
    return { error: `未找到 session: ${input}` };
  }

  if (prefixMatches.length === 1) {
    return { session: prefixMatches[0] };
  }

  return {
    error: `前缀 "${input}" 匹配到多个 session`,
    matches: prefixMatches,
  };
}

/**
 * Format the session list for WeChat display.
 *
 * Shows the short ID (first 8 chars), summary, and relative time.
 * Marks the current session with "← 当前".
 */
export function formatSessionList(
  sessions: SDKSessionInfo[],
  currentSessionId: string | null,
  basename: string,
): string {
  const lines: string[] = [`**claude**:${basename}`, '', '历史 session：'];

  if (sessions.length === 0) {
    lines.push('  （无）');
    lines.push('', '提示：发消息会自动创建新 session');
    return lines.join('\n');
  }

  for (const s of sessions) {
    const shortId = s.sessionId.slice(0, 8);
    const isCurrent = s.sessionId === currentSessionId;
    const marker = isCurrent ? '  ← 当前' : '';
    const time = formatRelativeTime(s.lastModified);
    const title = displayTitle(s);
    lines.push(`  · [${shortId}] ${title}  ${time}${marker}`);
  }

  lines.push('', '命令： /resume <id前缀> 切换');
  return lines.join('\n');
}

/**
 * Build the reply for a successful session switch.
 */
export function formatSwitchReply(
  basename: string,
  session: SDKSessionInfo,
): string {
  const shortId = session.sessionId.slice(0, 8);
  const time = formatRelativeTime(session.lastModified);
  const title = displayTitle(session);
  return [
    `**claude**:${basename}`,
    '',
    `已切换到 session [${shortId}...]`,
    `标题：${title}`,
    `上次活跃：${time}`,
    '下次消息将从该 session 恢复',
  ].join('\n');
}

/**
 * Pick a display title: customTitle > summary > firstPrompt > '(无标题)'.
 */
function displayTitle(s: SDKSessionInfo): string {
  const title = (s.customTitle || s.summary || s.firstPrompt || '').trim();
  if (!title) return '(无标题)';
  // Truncate long titles for WeChat display
  return title.length > 40 ? title.slice(0, 40) + '...' : title;
}

/**
 * Format a relative time string (e.g., "2小时前", "3天前").
 */
function formatRelativeTime(timestamp: number): string {
  const now = Date.now();
  const diffMs = now - timestamp;
  const diffSec = Math.floor(diffMs / 1000);
  const diffMin = Math.floor(diffSec / 60);
  const diffHour = Math.floor(diffMin / 60);
  const diffDay = Math.floor(diffHour / 24);

  if (diffMin < 1) return '刚刚';
  if (diffMin < 60) return `${diffMin}分钟前`;
  if (diffHour < 24) return `${diffHour}小时前`;
  if (diffDay < 30) return `${diffDay}天前`;
  return `${Math.floor(diffDay / 30)}个月前`;
}
