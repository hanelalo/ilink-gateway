import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import type { UserSessionData } from './session-store.js';
import fs from 'node:fs';
import os from 'node:os';

vi.mock('node:fs');

// These functions manipulate session data directly — we test the logic
// by calling the exported functions with mock session data.
import {
  resolvePath,
  formatStatus,
  formatCdError,
  buildSwitchReply,
  buildAddAliasReply,
  buildRemoveAliasReply,
  buildCloseReply,
  switchCwd,
  addAlias,
  removeAlias,
  closeWorkspace,
} from './cd-command.js';

function makeSession(overrides: Partial<UserSessionData> = {}): UserSessionData {
  return {
    aliases: {
      gw: '/Users/test/wechat-gateway',
      wiki: '/Users/test/wiki',
    },
    activeCwd: '/Users/test/wechat-gateway',
    sessions: {
      '/Users/test/wechat-gateway': {
        sessionId: 'uuid-gw-12345678',
        lastActive: 1000,
        approvedTools: ['Bash', 'Read'],
      },
      '/Users/test/wiki': {
        sessionId: 'uuid-wiki-abcdef',
        lastActive: 2000,
        approvedTools: ['Read', 'Glob'],
      },
    },
    ...overrides,
  };
}

describe('resolvePath', () => {
  it('should resolve alias', () => {
    const session = makeSession();
    expect(resolvePath(session, 'gw')).toBe('/Users/test/wechat-gateway');
    expect(resolvePath(session, 'wiki')).toBe('/Users/test/wiki');
  });

  it('should resolve by exact cwd match', () => {
    const session = makeSession();
    expect(resolvePath(session, '/Users/test/wechat-gateway')).toBe('/Users/test/wechat-gateway');
  });

  it('should resolve by fuzzy basename match (not alias)', () => {
    const session = makeSession({
      aliases: { gw: '/Users/test/wechat-gateway' },
      sessions: {
        '/Users/test/some-project': {
          sessionId: 'uuid-sp',
          lastActive: 1000,
          approvedTools: [],
        },
        '/Users/test/wiki': {
          sessionId: 'uuid-wiki-abcdef',
          lastActive: 2000,
          approvedTools: ['Read', 'Glob'],
        },
        '/Users/test/wechat-gateway': {
          sessionId: 'uuid-gw-12345678',
          lastActive: 1000,
          approvedTools: ['Bash', 'Read'],
        },
      },
    });
    // 'some-project' is not an alias — fuzzy match by basename
    expect(resolvePath(session, 'some-project')).toBe('/Users/test/some-project');
    expect(resolvePath(session, 'wechat-gateway')).toBe('/Users/test/wechat-gateway');
  });

  it('should resolve by absolute path only when directory exists', () => {
    const session = makeSession();
    vi.mocked(fs.existsSync).mockReturnValue(true);
    vi.mocked(fs.statSync).mockReturnValue({ isDirectory: () => true } as ReturnType<typeof fs.statSync>);
    expect(resolvePath(session, '/Users/test/other-project')).toBe('/Users/test/other-project');
  });

  it('should return error when absolute path does not exist', () => {
    const session = makeSession();
    vi.mocked(fs.existsSync).mockReturnValue(false);
    const result = resolvePath(session, '/Users/test/nonexistent');
    expect(result).toContain('路径不存在');
  });

  it('should return error when absolute path is not a directory', () => {
    const session = makeSession();
    vi.mocked(fs.existsSync).mockReturnValue(true);
    vi.mocked(fs.statSync).mockReturnValue({ isDirectory: () => false } as ReturnType<typeof fs.statSync>);
    const result = resolvePath(session, '/Users/test/file.txt');
    expect(result).toContain('路径不存在');
  });

  it('should expand tilde to homedir and resolve absolute path', () => {
    const session = makeSession();
    const homedir = os.homedir();
    const expected = `${homedir}/other-project`;
    vi.mocked(fs.existsSync).mockImplementation((p) => (p as string) === expected);
    vi.mocked(fs.statSync).mockReturnValue({ isDirectory: () => true } as ReturnType<typeof fs.statSync>);
    const result = resolvePath(session, '~/other-project');
    expect(result).toBe(expected);
  });

  it('should return error when tilde path does not exist', () => {
    const session = makeSession();
    vi.mocked(fs.existsSync).mockReturnValue(false);
    const result = resolvePath(session, '~/nonexistent');
    expect(result).toContain('路径不存在');
  });

  it('should return error message when no match found', () => {
    const session = makeSession();
    const result = resolvePath(session, 'unknown');
    expect(typeof result).toBe('string');
    expect(result).toContain('未找到');
    expect(result).toContain('unknown');
  });
});

describe('formatStatus', () => {
  it('should include current activeCwd', () => {
    const session = makeSession();
    const result = formatStatus(session);
    expect(result).toContain('**claude**:wechat-gateway');
    expect(result).toContain('wechat-gateway');
  });

  it('should list aliases', () => {
    const session = makeSession();
    const result = formatStatus(session);
    expect(result).toContain('gw');
    expect(result).toContain('wiki');
  });

  it('should include all workspaces sorted by lastActive desc', () => {
    const session = makeSession();
    const result = formatStatus(session);
    // wiki has lastActive 2000, gw has 1000 — wiki should come first
    // Find the workspace listing section
    const wsSection = result.slice(result.indexOf('所有 workspace'));
    const wikiIdx = wsSection.indexOf('wiki');
    const gwIdx = wsSection.indexOf('wechat-gateway');
    expect(wikiIdx).toBeLessThan(gwIdx);
  });

  it('should handle empty sessions', () => {
    const session = makeSession({ sessions: {} });
    const result = formatStatus(session);
    expect(result).toContain('当前');
  });
});

describe('formatCdError', () => {
  it('should format error with basename', () => {
    const result = formatCdError('wechat-gateway', '未找到项目: unknown');
    expect(result).toContain('**claude**:wechat-gateway');
    expect(result).toContain('未找到项目: unknown');
  });
});

describe('buildSwitchReply', () => {
  it('should include basename header and confirmation', () => {
    const session = makeSession();
    const sessionData = session.sessions['/Users/test/wiki'];
    const result = buildSwitchReply(
      '/Users/test/wiki',
      'wiki',
      sessionData,
    );
    expect(result).toContain('**claude**:wiki');
    expect(result).toContain('已切换到 wiki');
    expect(result).toContain('uuid-wik');
  });

  it('should indicate new session when sessionData is null or has no sessionId', () => {
    const result = buildSwitchReply('/Users/test/new-project', 'new-project', null);
    expect(result).toContain('新会话');
    expect(result).not.toContain('sessionId');
  });

  it('should indicate new session when sessionData has null sessionId', () => {
    const result = buildSwitchReply(
      '/Users/test/new-project',
      'new-project',
      { sessionId: null, lastActive: 0, approvedTools: [] },
    );
    expect(result).toContain('新会话');
  });
});

describe('buildAddAliasReply', () => {
  it('should include path and alias name', () => {
    const result = buildAddAliasReply('wiki', '/Users/test/wiki');
    expect(result).toContain('**claude**:wiki');
    expect(result).toContain('已添加别名');
    expect(result).toContain('wiki');
    expect(result).toContain('/Users/test/wiki');
  });
});

describe('buildRemoveAliasReply', () => {
  it('should include alias name', () => {
    const result = buildRemoveAliasReply('wiki', '/Users/test/wiki');
    expect(result).toContain('**claude**:wiki');
    expect(result).toContain('已删除别名');
    expect(result).toContain('wiki');
  });
});

describe('buildCloseReply', () => {
  it('should include workspace name', () => {
    const result = buildCloseReply('wiki');
    expect(result).toContain('**claude**:wiki');
    expect(result).toContain('已关闭 workspace');
    expect(result).toContain('wiki');
  });
});

describe('switchCwd', () => {
  it('should update activeCwd and return a reply', () => {
    const session = makeSession();
    const saveSpy = vi.fn();

    const reply = switchCwd(session, 'wiki', saveSpy);

    expect(session.activeCwd).toBe('/Users/test/wiki');
    expect(saveSpy).toHaveBeenCalledOnce();
    expect(reply).toContain('已切换到 wiki');
  });
});

describe('addAlias', () => {
  it('should add alias for current activeCwd when no path given', () => {
    const session = makeSession();
    const saveSpy = vi.fn();

    const reply = addAlias(session, 'myproj', undefined, saveSpy);

    expect(reply).toContain('已添加别名');
    expect(session.aliases['myproj']).toBe('/Users/test/wechat-gateway');
    expect(saveSpy).toHaveBeenCalledOnce();
  });

  it('should add alias for specified path', () => {
    const session = makeSession();
    const saveSpy = vi.fn();

    const reply = addAlias(session, 'other', '/some/other/path', saveSpy);

    expect(session.aliases['other']).toBe('/some/other/path');
    expect(saveSpy).toHaveBeenCalledOnce();
    expect(reply).toContain('已添加别名');
  });

  it('should reject invalid alias names', () => {
    const session = makeSession();
    const reply = addAlias(session, 'bad/name', undefined, vi.fn());
    expect(reply).toContain('错误');
  });
});

describe('removeAlias', () => {
  it('should remove existing alias', () => {
    const session = makeSession();
    const saveSpy = vi.fn();

    const reply = removeAlias(session, 'gw', saveSpy);

    expect(session.aliases).not.toHaveProperty('gw');
    expect(reply).toContain('已删除别名');
    expect(saveSpy).toHaveBeenCalledOnce();
  });

  it('should reject removal of default alias "."', () => {
    const session = makeSession({ aliases: { '.': '/test' } });
    const reply = removeAlias(session, '.', vi.fn());
    expect(reply).toContain('错误');
    expect(reply).toContain('.');
    expect(session.aliases['.']).toBe('/test');
  });

  it('should reject removal of non-existent alias', () => {
    const session = makeSession();
    const reply = removeAlias(session, 'nonexistent', vi.fn());
    expect(reply).toContain('错误');
    expect(reply).toContain('nonexistent');
  });
});

describe('closeWorkspace', () => {
  it('should clear session mapping and abort if running', () => {
    const session = makeSession();
    const saveSpy = vi.fn();
    const abortSpy = vi.fn();

    const reply = closeWorkspace(session, 'gw', { abort: abortSpy, save: saveSpy });

    expect(session.sessions).not.toHaveProperty('/Users/test/wechat-gateway');
    expect(abortSpy).toHaveBeenCalledWith('/Users/test/wechat-gateway');
    expect(saveSpy).toHaveBeenCalledOnce();
    expect(reply).toContain('已关闭 workspace');
  });

  it('should switch activeCwd to another workspace when closing current', () => {
    const session = makeSession();
    // Current activeCwd is /Users/test/wechat-gateway
    closeWorkspace(session, 'gw', { abort: vi.fn(), save: vi.fn() });

    // Should auto-switch to the remaining workspace
    expect(session.activeCwd).toBe('/Users/test/wiki');
  });

  it('should set activeCwd to empty when closing last workspace', () => {
    const session = makeSession({
      sessions: {
        '/Users/test/wechat-gateway': {
          sessionId: 'uuid-gw',
          lastActive: 1000,
          approvedTools: [],
        },
      },
    });
    closeWorkspace(session, 'gw', { abort: vi.fn(), save: vi.fn() });
    expect(session.activeCwd).toBe('');
  });
});
