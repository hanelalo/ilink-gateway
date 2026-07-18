import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {
  type UserSessionData,
  expandTilde,
  loadAll,
  saveAll,
} from './session-store.js';

describe('expandTilde', () => {
  it('should replace leading ~ with home directory', () => {
    const result = expandTilde('~/.wechat-gateway/sessions.json');
    expect(result).toBe(`${os.homedir()}/.wechat-gateway/sessions.json`);
  });

  it('should leave paths without tilde unchanged', () => {
    const result = expandTilde('/tmp/sessions.json');
    expect(result).toBe('/tmp/sessions.json');
  });

  it('should leave tilde in the middle of path unchanged', () => {
    const result = expandTilde('/home/~test/sessions.json');
    expect(result).toBe('/home/~test/sessions.json');
  });

  it('should handle just tilde', () => {
    const result = expandTilde('~');
    expect(result).toBe(os.homedir());
  });
});

describe('session-store loadAll / saveAll', () => {
  let tmpFile: string;
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'session-store-test-'));
    tmpFile = path.join(tmpDir, 'claude-sessions.json');
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('should return empty object when file does not exist', async () => {
    const data = await loadAll(tmpFile);
    expect(data).toEqual({});
  });

  it('should save and load data correctly', async () => {
    const testData: Record<string, UserSessionData> = {
      wxid_abc123: {
        aliases: {
          wiki: '/Users/test/develop/wiki',
          gw: '/Users/test/develop/wechat-gateway',
        },
        activeCwd: '/Users/test/develop/wiki',
        sessions: {
          '/Users/test/develop/wiki': {
            sessionId: 'uuid-1',
            lastActive: 1720000000,
            approvedTools: ['Bash', 'Read'],
          },
          '/Users/test/develop/wechat-gateway': {
            sessionId: 'uuid-2',
            lastActive: 1720000001,
            approvedTools: ['Glob'],
          },
        },
      },
    };

    await saveAll(testData, tmpFile);
    const loaded = await loadAll(tmpFile);

    expect(loaded).toEqual(testData);
  });

  it('should preserve empty sessions and aliases', async () => {
    const testData: Record<string, UserSessionData> = {
      wxid_empty: {
        aliases: {},
        activeCwd: '/tmp',
        sessions: {},
      },
    };

    await saveAll(testData, tmpFile);
    const loaded = await loadAll(tmpFile);
    expect(loaded).toEqual(testData);
  });

  it('should handle multiple wxid entries', async () => {
    const testData: Record<string, UserSessionData> = {
      wxid_a: {
        aliases: { a: '/tmp/a' },
        activeCwd: '/tmp/a',
        sessions: {
          '/tmp/a': {
            sessionId: 'uuid-a',
            lastActive: 1000,
            approvedTools: [],
          },
        },
      },
      wxid_b: {
        aliases: { b: '/tmp/b' },
        activeCwd: '/tmp/b',
        sessions: {
          '/tmp/b': {
            sessionId: 'uuid-b',
            lastActive: 2000,
            approvedTools: ['Read', 'Glob'],
          },
        },
      },
    };

    await saveAll(testData, tmpFile);
    const loaded = await loadAll(tmpFile);
    expect(loaded).toEqual(testData);
  });

  it('should overwrite file on subsequent saves', async () => {
    const first: Record<string, UserSessionData> = {
      wxid_x: {
        aliases: {},
        activeCwd: '/tmp/first',
        sessions: {},
      },
    };
    await saveAll(first, tmpFile);

    const second: Record<string, UserSessionData> = {
      wxid_y: {
        aliases: {},
        activeCwd: '/tmp/second',
        sessions: {},
      },
    };
    await saveAll(second, tmpFile);

    const loaded = await loadAll(tmpFile);
    expect(loaded).toEqual(second);
    expect(loaded).not.toHaveProperty('wxid_x');
  });

  it('should throw on EACCES (permission error)', async () => {
    // Write a file first, then deny read access
    await saveAll({ test: { aliases: {}, activeCwd: '/tmp', sessions: {} } }, tmpFile);

    // Temporarily remove read permission
    fs.chmodSync(tmpFile, 0o000);

    await expect(loadAll(tmpFile)).rejects.toThrow();

    fs.chmodSync(tmpFile, 0o644);
  });

  it('should throw on unknown errors', async () => {
    // Trigger an error by providing a directory instead of a file
    await expect(loadAll(tmpDir)).rejects.toThrow();
  });

  it('should return empty object with console.warn on JSON parse error (corrupt JSON)', async () => {
    await fs.promises.writeFile(tmpFile, 'not valid json', 'utf-8');
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

    const data = await loadAll(tmpFile);
    expect(data).toEqual({});
    expect(warnSpy).toHaveBeenCalled();
    expect(warnSpy.mock.calls[0][0]).toContain('corrupt');

    warnSpy.mockRestore();
  });

  it('should handle sessionId being null for new sessions', async () => {
    const testData: Record<string, UserSessionData> = {
      wxid_new: {
        aliases: {},
        activeCwd: '/tmp/new',
        sessions: {
          '/tmp/new': {
            sessionId: null,
            lastActive: Date.now(),
            approvedTools: ['Read', 'Glob', 'Grep'],
          },
        },
      },
    };

    await saveAll(testData, tmpFile);
    const loaded = await loadAll(tmpFile);
    expect(loaded.wxid_new.sessions['/tmp/new'].sessionId).toBeNull();
  });
});
