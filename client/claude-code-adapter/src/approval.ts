/**
 * Tool approval module — parses WeChat approval commands, manages timeout promises,
 * and formats approval prompts for display in WeChat.
 */

export interface ParsedApprovalCommand {
  type: 'approve' | 'deny' | 'approve_session' | 'approve_on' | 'approve_off';
}

/**
 * Parse a WeChat message to determine if it is an approval command.
 *
 * Supported commands (case-insensitive):
 *   /approve          — approve the current tool call
 *   /deny             — deny the current tool call
 *   /approve session  — approve and remember the tool for this session
 *   /approve on       — switch to auto-approve mode
 *   /approve off      — switch to interactive approve mode
 *
 * Returns null when the text is not a recognized approval command.
 */
export function parseApprovalCommand(text: string): ParsedApprovalCommand | null {
  const trimmed = text.trim();
  const lower = trimmed.toLowerCase();

  if (/^\/approve\s+session\b/.test(lower)) {
    return { type: 'approve_session' };
  }

  if (/^\/approve\s+on\b/.test(lower)) {
    return { type: 'approve_on' };
  }

  if (/^\/approve\s+off\b/.test(lower)) {
    return { type: 'approve_off' };
  }

  if (/^\/approve\b/.test(lower)) {
    return { type: 'approve' };
  }

  if (/^\/deny\b/.test(lower)) {
    return { type: 'deny' };
  }

  return null;
}

/**
 * Create a promise that resolves to `false` after a timeout, representing an
 * automatically denied tool approval when the user does not respond in time.
 */
export function createApprovalTimeout(timeoutMs = 60000): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    setTimeout(() => {
      resolve(false);
    }, timeoutMs);
  });
}

/**
 * Format a human-readable approval prompt for WeChat display.
 *
 * Example output:
 * ```
 * **claude**:wiki
 *
 * Claude 想执行 Bash: npm install
 * 回复: /approve  /deny  /approve session
 * ```
 */
export function formatApprovalPrompt(
  workspace: string,
  toolName: string,
  toolInput: unknown,
): string {
  const inputStr =
    typeof toolInput === 'object' && toolInput !== null
      ? JSON.stringify(toolInput, null, 0)
      : String(toolInput ?? '');

  return [
    `**claude**:${workspace}`,
    '',
    `Claude 想执行 ${toolName}: ${inputStr}`,
    '回复: /approve  /deny  /approve session',
  ].join('\n');
}
