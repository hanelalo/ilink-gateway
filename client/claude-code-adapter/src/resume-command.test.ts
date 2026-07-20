import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { SDKSessionInfo } from '@anthropic-ai/claude-agent-sdk';

// Mock the SDK's listSessions so tests don't touch the real filesystem.
vi.mock('@anthropic-ai/claude-agent-sdk', () => ({
  listSessions: vi.fn(),
}));

import { listSessions } from '@anthropic-ai/claude-agent-sdk';
import {
  listSessionsForCwd,
  resolveSession,
  formatSessionList,
  formatSwitchReply,
} from './resume-command.js';

function makeSession(overrides: Partial<SDKSessionInfo> = {}): SDKSessionInfo {
  return {
    sessionId: 'abcdef01-2345-6789-abcd-ef0123456789',
    summary: 'Fix login bug',
    lastModified: Date.now() - 3600_000,
    ...overrides,
  };
}

describe('listSessionsForCwd', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('calls SDK listSessions with the given dir', async () => {
    const mockSessions = [makeSession(), makeSession({ sessionId: '99999999-aaaa-bbbb-cccc-dddddddddddd' })];
    vi.mocked(listSessions).mockResolvedValue(mockSessions);

    const result = await listSessionsForCwd('/path/to/project');

    expect(listSessions).toHaveBeenCalledWith({ dir: '/path/to/project' });
    expect(result).toHaveLength(2);
  });

  it('sorts sessions by lastModified descending', async () => {
    const older = makeSession({ sessionId: 'older', lastModified: 1000 });
    const newer = makeSession({ sessionId: 'newer', lastModified: 5000 });
    vi.mocked(listSessions).mockResolvedValue([older, newer]);

    const result = await listSessionsForCwd('/path');

    expect(result[0].sessionId).toBe('newer');
    expect(result[1].sessionId).toBe('older');
  });

  it('does not mutate the original array from SDK', async () => {
    const original = [makeSession({ lastModified: 1000 }), makeSession({ lastModified: 5000 })];
    vi.mocked(listSessions).mockResolvedValue(original);

    await listSessionsForCwd('/path');

    // Original array order unchanged
    expect(original[0].lastModified).toBe(1000);
  });
});

describe('resolveSession', () => {
  const sessions: SDKSessionInfo[] = [
    makeSession({ sessionId: 'abcdef01-2345-6789-abcd-ef0123456789' }),
    makeSession({ sessionId: 'bbbbbbbb-3333-4444-5555-666666666666' }),
    makeSession({ sessionId: 'abababab-9999-8888-7777-666666666666' }),
  ];

  it('exact match (case-insensitive)', () => {
    const result = resolveSession('ABCDEF01-2345-6789-ABCD-EF0123456789', sessions);
    expect(result.session?.sessionId).toBe('abcdef01-2345-6789-abcd-ef0123456789');
  });

  it('unique prefix match', () => {
    const result = resolveSession('bbbb', sessions);
    expect(result.session?.sessionId).toBe('bbbbbbbb-3333-4444-5555-666666666666');
  });

  it('ambiguous prefix returns matches', () => {
    // Both "abcdef..." and "ababab..." start with "ab"
    const result = resolveSession('ab', sessions);
    expect(result.error).toContain('多个');
    expect(result.matches).toHaveLength(2);
  });

  it('no match returns error', () => {
    const result = resolveSession('nonexistent', sessions);
    expect(result.error).toContain('未找到');
    expect(result.session).toBeUndefined();
  });

  it('empty input returns error', () => {
    const result = resolveSession('   ', sessions);
    expect(result.error).toContain('不能为空');
  });

  it('handles empty session list', () => {
    const result = resolveSession('abc', []);
    expect(result.error).toBeDefined();
  });
});

describe('formatSessionList', () => {
  it('empty list shows hint', () => {
    const result = formatSessionList([], null, 'myproject');
    expect(result).toContain('**claude**:myproject');
    expect(result).toContain('（无）');
    expect(result).toContain('发消息会自动创建新 session');
  });

  it('marks current session', () => {
    const currentId = 'abcdef01-2345-6789-abcd-ef0123456789';
    const sessions = [
      makeSession({ sessionId: currentId, summary: 'Current work' }),
      makeSession({ sessionId: 'other-id-1234567890', summary: 'Past work' }),
    ];
    const result = formatSessionList(sessions, currentId, 'proj');
    expect(result).toContain('← 当前');
    // Current session comes first in the list (assuming already sorted)
    expect(result.indexOf('abcdef01')).toBeLessThan(result.indexOf('other-id'));
  });

  it('shows short ID (first 8 chars) and title', () => {
    const sessions = [
      makeSession({
        sessionId: 'abcdef01-2345-6789-abcd-ef0123456789',
        summary: 'My session title',
      }),
    ];
    const result = formatSessionList(sessions, null, 'proj');
    expect(result).toContain('[abcdef01]');
    expect(result).toContain('My session title');
  });

  it('uses customTitle over summary', () => {
    const sessions = [
      makeSession({
        sessionId: 'abcdef01-2345-6789-abcd-ef0123456789',
        summary: 'auto summary',
        customTitle: 'custom name',
      }),
    ];
    const result = formatSessionList(sessions, null, 'proj');
    expect(result).toContain('custom name');
    expect(result).not.toContain('auto summary');
  });

  it('falls back to firstPrompt when summary empty', () => {
    const sessions = [
      makeSession({
        sessionId: 'abcdef01-2345',
        summary: '',
        firstPrompt: 'How do I do X?',
      }),
    ];
    const result = formatSessionList(sessions, null, 'proj');
    expect(result).toContain('How do I do X?');
  });

  it('shows (无标题) when no title fields', () => {
    const sessions = [
      makeSession({
        sessionId: 'abcdef01-2345',
        summary: '',
        customTitle: undefined,
        firstPrompt: undefined,
      }),
    ];
    const result = formatSessionList(sessions, null, 'proj');
    expect(result).toContain('(无标题)');
  });

  it('truncates long titles', () => {
    const longTitle = 'A'.repeat(100);
    const sessions = [
      makeSession({ sessionId: 'abcdef01-2345', summary: longTitle }),
    ];
    const result = formatSessionList(sessions, null, 'proj');
    expect(result).toContain('...');
    // Should not contain the full 100-char title
    expect(result).not.toContain(longTitle);
  });

  it('includes command hint at the bottom', () => {
    const sessions = [makeSession()];
    const result = formatSessionList(sessions, null, 'proj');
    expect(result).toContain('/resume <id前缀>');
  });
});

describe('formatSwitchReply', () => {
  it('shows short ID, title, and time', () => {
    const session = makeSession({
      sessionId: 'abcdef01-2345-6789-abcd-ef0123456789',
      summary: 'My work',
      lastModified: Date.now() - 7200_000, // 2 hours ago
    });
    const result = formatSwitchReply('proj', session);
    expect(result).toContain('**claude**:proj');
    expect(result).toContain('[abcdef01...]');
    expect(result).toContain('My work');
    expect(result).toContain('2小时前');
    expect(result).toContain('下次消息将从该 session 恢复');
  });
});
