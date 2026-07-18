import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { QueryManager } from './query-manager.js';

describe('QueryManager', () => {
  let qm: QueryManager;

  beforeEach(() => {
    vi.useFakeTimers();
    qm = new QueryManager();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('start', () => {
    it('should create a new running query for a (wxid, cwd) pair', () => {
      const query = qm.start('wxid_a', '/home/project');

      expect(query.abortController).toBeInstanceOf(AbortController);
      expect(query.pendingApproval).toBeNull();
      expect(query.messageQueue).toEqual([]);
    });

    it('should abort the previous query when starting the same (wxid, cwd)', () => {
      const first = qm.start('wxid_a', '/home/project');
      const abortSpy = vi.spyOn(first.abortController, 'abort');

      const second = qm.start('wxid_a', '/home/project');

      expect(abortSpy).toHaveBeenCalledTimes(1);
      expect(qm.get('wxid_a', '/home/project')).toBe(second);
      expect(qm.get('wxid_a', '/home/project')).not.toBe(first);
    });

    it('should clear the pending approval timer of the previous query', () => {
      const first = qm.start('wxid_a', '/home/project');
      const timer = setTimeout(() => {}, 10_000);
      const clearSpy = vi.spyOn(globalThis, 'clearTimeout');

      qm.setPendingApproval('wxid_a', '/home/project', {
        toolName: 'Bash',
        resolver: vi.fn(),
        timer,
      });

      clearSpy.mockClear();
      qm.start('wxid_a', '/home/project');

      expect(clearSpy).toHaveBeenCalledWith(timer);
    });

    it('should allow different wxid with the same cwd to coexist', () => {
      const q1 = qm.start('wxid_a', '/home/project');
      const q2 = qm.start('wxid_b', '/home/project');

      expect(q1).not.toBe(q2);
      expect(qm.get('wxid_a', '/home/project')).toBe(q1);
      expect(qm.get('wxid_b', '/home/project')).toBe(q2);
    });

    it('should allow the same wxid with different cwd to coexist', () => {
      const q1 = qm.start('wxid_a', '/project/a');
      const q2 = qm.start('wxid_a', '/project/b');

      expect(q1).not.toBe(q2);
      expect(qm.get('wxid_a', '/project/a')).toBe(q1);
      expect(qm.get('wxid_a', '/project/b')).toBe(q2);
    });
  });

  describe('get', () => {
    it('should return undefined for a non-existent pair', () => {
      expect(qm.get('wxid_x', '/nowhere')).toBeUndefined();
    });

    it('should return the RunningQuery for an existing pair', () => {
      const query = qm.start('wxid_a', '/home');
      expect(qm.get('wxid_a', '/home')).toBe(query);
    });

    it('should return undefined if only wxid exists but with different cwd', () => {
      qm.start('wxid_a', '/home');
      expect(qm.get('wxid_a', '/other')).toBeUndefined();
    });
  });

  describe('remove', () => {
    it('should remove an existing query and return true', () => {
      qm.start('wxid_a', '/home');
      const removed = qm.remove('wxid_a', '/home');

      expect(removed).toBe(true);
      expect(qm.get('wxid_a', '/home')).toBeUndefined();
    });

    it('should clear the pending approval timer before removing', () => {
      const query = qm.start('wxid_a', '/home');
      const timer = setTimeout(() => {}, 10_000);
      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Bash',
        resolver: vi.fn(),
        timer,
      });

      const clearSpy = vi.spyOn(globalThis, 'clearTimeout');
      qm.remove('wxid_a', '/home');

      expect(clearSpy).toHaveBeenCalledWith(timer);
      expect(query.pendingApproval).toBeNull();
    });

    it('should return false for a non-existent pair', () => {
      expect(qm.remove('wxid_x', '/nowhere')).toBe(false);
    });

    it('should not affect other pairs for the same wxid', () => {
      qm.start('wxid_a', '/project/a');
      qm.start('wxid_a', '/project/b');

      qm.remove('wxid_a', '/project/a');

      expect(qm.get('wxid_a', '/project/a')).toBeUndefined();
      expect(qm.get('wxid_a', '/project/b')).toBeDefined();
    });

    it('should return false when only wxid exists with different cwd', () => {
      qm.start('wxid_a', '/home');
      expect(qm.remove('wxid_a', '/other')).toBe(false);
    });
  });

  describe('abort', () => {
    it('should abort the abort controller and remove the query', () => {
      const query = qm.start('wxid_a', '/home');
      const abortSpy = vi.spyOn(query.abortController, 'abort');

      const result = qm.abort('wxid_a', '/home');

      expect(abortSpy).toHaveBeenCalledTimes(1);
      expect(result).toBe(true);
      expect(qm.get('wxid_a', '/home')).toBeUndefined();
    });

    it('should clear the pending approval timer before aborting', () => {
      qm.start('wxid_a', '/home');
      const timer = setTimeout(() => {}, 10_000);
      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Bash',
        resolver: vi.fn(),
        timer,
      });

      const clearSpy = vi.spyOn(globalThis, 'clearTimeout');
      qm.abort('wxid_a', '/home');

      expect(clearSpy).toHaveBeenCalledWith(timer);
    });

    it('should return false for a non-existent pair', () => {
      expect(qm.abort('wxid_x', '/nowhere')).toBe(false);
    });
  });

  describe('setPendingApproval', () => {
    it('should set pending approval on an existing query', () => {
      const query = qm.start('wxid_a', '/home');
      const resolver = vi.fn();
      const timer = setTimeout(() => {}, 1000);

      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Bash',
        resolver,
        timer,
      });

      expect(query.pendingApproval).toEqual({
        toolName: 'Bash',
        resolver,
        timer,
      });
    });

    it('should clear the previous timer when setting a new approval', () => {
      const query = qm.start('wxid_a', '/home');
      const oldTimer = setTimeout(() => {}, 10_000);
      const clearSpy = vi.spyOn(globalThis, 'clearTimeout');

      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Bash',
        resolver: vi.fn(),
        timer: oldTimer,
      });

      clearSpy.mockClear();

      const newTimer = setTimeout(() => {}, 10_000);
      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Read',
        resolver: vi.fn(),
        timer: newTimer,
      });

      expect(clearSpy).toHaveBeenCalledWith(oldTimer);
      expect(query.pendingApproval!.timer).toBe(newTimer);
      expect(query.pendingApproval!.toolName).toBe('Read');
    });

    it('should be a no-op for a non-existent pair', () => {
      expect(() => {
        qm.setPendingApproval('wxid_x', '/nowhere', {
          toolName: 'Bash',
          resolver: vi.fn(),
          timer: setTimeout(() => {}, 1000),
        });
      }).not.toThrow();
    });
  });

  describe('abortAll', () => {
    it('should abort all queries across all users', () => {
      const q1 = qm.start('wxid_a', '/home');
      const q2 = qm.start('wxid_a', '/other');
      const q3 = qm.start('wxid_b', '/home');

      const abortSpy1 = vi.spyOn(q1.abortController, 'abort');
      const abortSpy2 = vi.spyOn(q2.abortController, 'abort');
      const abortSpy3 = vi.spyOn(q3.abortController, 'abort');

      qm.abortAll();

      expect(abortSpy1).toHaveBeenCalledTimes(1);
      expect(abortSpy2).toHaveBeenCalledTimes(1);
      expect(abortSpy3).toHaveBeenCalledTimes(1);

      expect(qm.get('wxid_a', '/home')).toBeUndefined();
      expect(qm.get('wxid_a', '/other')).toBeUndefined();
      expect(qm.get('wxid_b', '/home')).toBeUndefined();
    });

    it('should resolve pending approvals with false', () => {
      const resolver1 = vi.fn();
      const resolver2 = vi.fn();

      qm.start('wxid_a', '/home');
      qm.start('wxid_b', '/other');

      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Bash',
        resolver: resolver1,
        timer: setTimeout(() => {}, 10_000),
      });
      qm.setPendingApproval('wxid_b', '/other', {
        toolName: 'Read',
        resolver: resolver2,
        timer: setTimeout(() => {}, 10_000),
      });

      qm.abortAll();

      expect(resolver1).toHaveBeenCalledWith(false);
      expect(resolver2).toHaveBeenCalledWith(false);
    });

    it('should clear store after aborting', () => {
      qm.start('wxid_a', '/home');
      qm.start('wxid_a', '/other');

      qm.abortAll();

      expect(qm['store'].size).toBe(0);
    });

    it('should be a no-op when no queries exist', () => {
      expect(() => qm.abortAll()).not.toThrow();
    });
  });

  describe('clearPendingApproval', () => {
    it('should clear the pending approval and timer on an existing query', () => {
      const query = qm.start('wxid_a', '/home');
      const timer = setTimeout(() => {}, 1000);

      qm.setPendingApproval('wxid_a', '/home', {
        toolName: 'Bash',
        resolver: vi.fn(),
        timer,
      });
      expect(query.pendingApproval).not.toBeNull();

      const clearSpy = vi.spyOn(globalThis, 'clearTimeout');
      qm.clearPendingApproval('wxid_a', '/home');

      expect(clearSpy).toHaveBeenCalledWith(timer);
      expect(query.pendingApproval).toBeNull();
    });

    it('should be a no-op if no pending approval exists', () => {
      qm.start('wxid_a', '/home');
      expect(() => {
        qm.clearPendingApproval('wxid_a', '/home');
      }).not.toThrow();
    });

    it('should be a no-op for a non-existent pair', () => {
      expect(() => {
        qm.clearPendingApproval('wxid_x', '/nowhere');
      }).not.toThrow();
    });
  });
});
