/**
 * /cd command handler for switching workspaces in a WeChat Claude session.
 *
 * Implements the command parsing, path resolution, alias management,
 * and workspace switching logic described in the design doc §4.6.
 */

import type { UserSessionData, SessionEntry } from './session-store.js';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

export interface CloseWorkspaceCallbacks {
  abort: (cwd: string) => void;
  save: () => void;
}

/**
 * Resolve a user-provided target to an absolute directory path.
 *
 * Resolution order:
 * 1. Alias lookup
 * 2. Exact match against known session cwds
 * 3. Fuzzy match (basename) against known session cwds
 * 4. Treat as absolute path (for paths starting with "/")
 *
 * Returns the resolved path string on success, or an error message string.
 */
export function resolvePath(session: UserSessionData, target: string): string {
  // 1. Check aliases
  if (session.aliases[target]) {
    return session.aliases[target];
  }

  const knownCwds = Object.keys(session.sessions);

  // 2. Exact match
  const exactMatch = knownCwds.find((cwd) => cwd === target);
  if (exactMatch) {
    return exactMatch;
  }

  // 3. Fuzzy match by basename
  const fuzzyMatch = knownCwds.find((cwd) => path.basename(cwd) === target);
  if (fuzzyMatch) {
    return fuzzyMatch;
  }

  // 4. Absolute path - validate existence
  if (target.startsWith('/') || target.startsWith('~')) {
    const expanded = target.startsWith('~')
      ? target.replace(/^~/, os.homedir())
      : target;
    if (fs.existsSync(expanded) && fs.statSync(expanded).isDirectory()) {
      return expanded;
    }
    return `路径不存在或不是目录: ${target}`;
  }

  // No match found
  return `未找到项目：${target}`;
}

/**
 * Build the /cd status display showing aliases, current workspace, and
 * all sessions sorted by lastActive (most recent first).
 */
export function formatStatus(session: UserSessionData): string {
  const active = session.activeCwd;
  const activeBasename = active ? path.basename(active) : '(无)';

  // Collect aliases excluding the default "."
  const aliasEntries = Object.entries(session.aliases).filter(
    ([name]) => name !== '.',
  );
  const aliasList = aliasEntries.map(([name]) => name).join(', ');

  const lines: string[] = [
    `**claude**:${activeBasename}`,
    '',
    `当前：${activeBasename}    /cd 可切换项目`,
  ];

  if (aliasList) {
    lines.push(`可用：${aliasList}`);
  }

  lines.push('', '所有 workspace：');

  // Sort sessions by lastActive descending
  const sorted = Object.entries(session.sessions).sort(
    ([, a], [, b]) => b.lastActive - a.lastActive,
  );

  for (const [cwd, sess] of sorted) {
    const basename = path.basename(cwd);
    const isCurrent = cwd === active;
    const sid = sess.sessionId ? sess.sessionId.slice(0, 8) : '新';
    const lastTime = formatRelativeTime(sess.lastActive);
    const marker = isCurrent ? '  ← 当前' : '';
    lines.push(`  · ${basename} [${sid}] ${lastTime}${marker}`);
  }

  lines.push('', '命令： /cd name 切换  /cd +name 别名  /cd close name 关闭');
  return lines.join('\n');
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

/**
 * Format an error message with the workspace header.
 */
export function formatCdError(workspace: string, errorMsg: string): string {
  return [`**claude**:${workspace}`, '', errorMsg].join('\n');
}

/**
 * Build the reply for a successful workspace switch.
 */
export function buildSwitchReply(
  cwd: string,
  basename: string,
  sessionData: SessionEntry | null | undefined,
): string {
  const header = `**claude**:${basename}`;

  if (!sessionData || !sessionData.sessionId) {
    return [
      header,
      '',
      `已切换到 ${basename} [新会话]`,
      '下次消息时将创建 Claude session',
    ].join('\n');
  }

  const sid = sessionData.sessionId.slice(0, 8);
  const lastTime = formatRelativeTime(sessionData.lastActive);
  return [
    header,
    '',
    `已切换到 ${basename} [${sid}...]`,
    `上次活跃：${lastTime}`,
  ].join('\n');
}

/**
 * Build the reply for a successful alias addition.
 */
export function buildAddAliasReply(alias: string, resolvedPath: string): string {
  const basename = path.basename(resolvedPath);
  return [
    `**claude**:${basename}`,
    '',
    `已添加别名：${alias} = ${resolvedPath}`,
  ].join('\n');
}

/**
 * Build the reply for a successful alias removal.
 */
export function buildRemoveAliasReply(alias: string, resolvedPath: string): string {
  const basename = path.basename(resolvedPath);
  return [
    `**claude**:${basename}`,
    '',
    `已删除别名：${alias}`,
  ].join('\n');
}

/**
 * Build the reply for a successful workspace close.
 */
export function buildCloseReply(basename: string): string {
  return [
    `**claude**:${basename}`,
    '',
    `已关闭 workspace: ${basename}`,
  ].join('\n');
}

/**
 * Switch the active workspace for a user.
 * Updates activeCwd and saves the session data.
 */
export function switchCwd(
  session: UserSessionData,
  target: string,
  save: () => void,
): string {
  const resolved = resolvePath(session, target);

  // If resolved is an error message, return it
  if (resolved.startsWith('未找到') || resolved.startsWith('路径不存在')) {
    return formatCdError(path.basename(session.activeCwd), resolved);
  }

  session.activeCwd = resolved;
  save();

  const basename = path.basename(resolved);
  const sessionData = session.sessions[resolved];
  return buildSwitchReply(resolved, basename, sessionData);
}

/**
 * Add an alias for the current activeCwd or a specified path.
 */
export function addAlias(
  session: UserSessionData,
  name: string,
  explicitPath: string | undefined,
  save: () => void,
): string {
  if (!name || name.includes('/') || name.includes(' ')) {
    return formatCdError(
      path.basename(session.activeCwd),
      '错误：别名只能包含字母、数字、连字符',
    );
  }

  const resolvedPath = explicitPath || session.activeCwd;
  session.aliases[name] = resolvedPath;
  save();

  return buildAddAliasReply(name, resolvedPath);
}

/**
 * Remove an alias.
 */
export function removeAlias(
  session: UserSessionData,
  name: string,
  save: () => void,
): string {
  if (name === '.') {
    return formatCdError(
      path.basename(session.activeCwd),
      '错误：不能删除默认别名 "."',
    );
  }

  if (!(name in session.aliases)) {
    return formatCdError(
      path.basename(session.activeCwd),
      `错误：别名不存在：${name}`,
    );
  }

  const resolvedPath = session.aliases[name];
  const basename = path.basename(resolvedPath);
  delete session.aliases[name];
  save();

  return buildRemoveAliasReply(name, resolvedPath);
}

/**
 * Close a workspace: abort any running query, remove session data,
 * and auto-switch to another workspace if closing the active one.
 */
export function closeWorkspace(
  session: UserSessionData,
  target: string,
  callbacks: CloseWorkspaceCallbacks,
): string {
  const resolved = resolvePath(session, target);

  if (resolved.startsWith('未找到') || resolved.startsWith('路径不存在')) {
    return formatCdError(path.basename(session.activeCwd), resolved);
  }

  // Abort running query
  callbacks.abort(resolved);

  // Remove session data
  delete session.sessions[resolved];

  // If closing the active workspace, switch to another
  if (session.activeCwd === resolved) {
    const remaining = Object.keys(session.sessions);
    session.activeCwd = remaining.length > 0 ? remaining[0] : '';
  }

  callbacks.save();

  const basename = path.basename(resolved);
  return buildCloseReply(basename);
}
