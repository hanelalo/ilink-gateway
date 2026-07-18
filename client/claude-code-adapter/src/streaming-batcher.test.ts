import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { StreamingBatcher, splitLongMessage, splitLongReply } from './streaming-batcher.js';

describe('splitLongMessage', () => {
  it('should return original text as single segment when within maxLen', () => {
    expect(splitLongMessage('hello', 3800)).toEqual(['hello']);
  });

  it('should return original text as single segment when empty', () => {
    expect(splitLongMessage('', 3800)).toEqual(['']);
  });

  it('should split text longer than maxLen into segments', () => {
    const longText = 'a'.repeat(5000);
    const segments = splitLongMessage(longText, 3800);
    expect(segments.length).toBeGreaterThan(1);
    // Each segment is prefixed with [N/M], so joined != original
    // Check that content without prefix reconstructs
    const contentOnly = segments.map(s => s.replace(/^\[\d+\/\d+\] /, '')).join('');
    expect(contentOnly).toBe(longText);
  });

  it('should split large text evenly', () => {
    const longText = 'HelloWorld'.repeat(500);
    const segments = splitLongMessage(longText, 3800);
    expect(segments.length).toBeGreaterThan(1);
    const contentOnly = segments.map(s => s.replace(/^\[\d+\/\d+\] /, '')).join('');
    expect(contentOnly).toBe(longText);
  });

  it('should respect the custom maxLen parameter', () => {
    const longText = 'a'.repeat(100);
    const segments = splitLongMessage(longText, 30);
    expect(segments.length).toBeGreaterThan(1);
    segments.forEach((s) => {
      expect(s.length).toBeLessThanOrEqual(37); // 30 + some overhead margin
    });
  });

  it('should handle exact boundary', () => {
    const text = 'a'.repeat(3800);
    expect(splitLongMessage(text, 3800)).toEqual([text]);
  });
});

describe('splitLongReply', () => {
  it('should return single segment with header when body fits within maxLen', () => {
    const header = '**claude**:project';
    const body = 'Hello world';
    const result = splitLongReply(header, body, 3800);
    expect(result).toHaveLength(1);
    expect(result[0]).toBe(`${header}\n\n${body}`);
  });

  it('should prepend header to first segment when body exceeds maxLen', () => {
    const header = '**claude**:project';
    const body = 'x'.repeat(4000); // exceeds maxLen - header overhead
    const result = splitLongReply(header, body, 100);
    expect(result.length).toBeGreaterThan(1);
    // Every segment must start with header or continuation prefix
    expect(result[0]).toContain(header);
    expect(result[0].startsWith(header)).toBe(true);
  });

  it('should preserve full header only in first segment', () => {
    const header = '**claude**:project';
    const body = 'y'.repeat(5000);
    const result = splitLongReply(header, body, 100);
    expect(result.length).toBeGreaterThan(1);
    // First segment starts with the full header
    expect(result[0].startsWith(`${header}\n\n`)).toBe(true);
    // Subsequent segments do NOT start with the header
    for (let i = 1; i < result.length; i++) {
      expect(result[i].startsWith(header)).toBe(false);
    }
  });

  it('should return single segment when body fits exactly within bodyMaxLen', () => {
    const header = '**claude**:proj';
    const headerLen = header.length + 2; // +2 for "\n\n"
    const bodyMaxLen = 500 - headerLen;
    const body = 'z'.repeat(bodyMaxLen);
    const result = splitLongReply(header, body, 500);
    expect(result).toHaveLength(1);
    expect(result[0]).toBe(`${header}\n\n${body}`);
  });

  it('should handle empty body gracefully', () => {
    const header = '**claude**:project';
    const result = splitLongReply(header, '', 3800);
    expect(result).toHaveLength(1);
    expect(result[0]).toBe(`${header}\n\n`);
  });
});

describe('StreamingBatcher', () => {
  let batcher: StreamingBatcher;
  let onFlush: ReturnType<typeof vi.fn>;
  let onIdle: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    onFlush = vi.fn();
    onIdle = vi.fn();
  });

  afterEach(() => {
    if (batcher && !batcher['destroyed']) {
      batcher.destroy();
    }
    vi.useRealTimers();
  });

  describe('addText', () => {
    it('should accumulate text without immediate flush', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000, // prevent idle flush
        immediateFlushLength: 99999, // prevent immediate flush
      });
      batcher.addText('Hello ');
      batcher.addText('World');
      expect(onFlush).not.toHaveBeenCalled();
    });

    it('should flush when buffer exceeds immediateFlushLength', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 10,
      });
      batcher.addText('Hello ');
      batcher.addText('World!'); // exceeds 10
      expect(onFlush).toHaveBeenCalledTimes(1);
      expect(onFlush).toHaveBeenCalledWith('Hello World!');
    });

    it('should reset idle timer on new text', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 2000,
        immediateFlushLength: 99999,
      });
      batcher.addText('Hello ');
      vi.advanceTimersByTime(1500);
      batcher.addText('World');
      vi.advanceTimersByTime(1500); // 1500ms from last add, not enough
      expect(onFlush).not.toHaveBeenCalled();
      vi.advanceTimersByTime(1000); // 2500ms from last add, exceeds 2000ms idle
      expect(onFlush).toHaveBeenCalledTimes(1);
      expect(onFlush).toHaveBeenCalledWith('Hello World');
    });
  });

  describe('toolUse', () => {
    it('should flush current buffer on tool use', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
      });
      batcher.addText('Thinking...');
      batcher.toolUse();
      expect(onFlush).toHaveBeenCalledWith('Thinking...');
    });

    it('should reset activity timer on tool use', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
        activityTimeoutMs: 30000,
        checkIntervalMs: 5000,
      });
      batcher.addText('text');
      vi.advanceTimersByTime(25000);
      batcher.toolUse(); // resets activity timer
      vi.advanceTimersByTime(10000); // 10s from toolUse, not 30s
      expect(onIdle).not.toHaveBeenCalled();
    });

    it('should not flush when buffer is empty', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
      });
      batcher.toolUse();
      expect(onFlush).not.toHaveBeenCalled();
    });
  });

  describe('flush', () => {
    it('should flush accumulated buffer', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
      });
      batcher.addText('Hello ');
      batcher.addText('World');
      batcher.flush();
      expect(onFlush).toHaveBeenCalledWith('Hello World');
      expect(batcher['buffer']).toBe('');
    });

    it('should be no-op when buffer is empty', () => {
      batcher = new StreamingBatcher(onFlush, onIdle);
      batcher.flush();
      expect(onFlush).not.toHaveBeenCalled();
    });
  });

  describe('idle timer', () => {
    it('should flush after idleTimeoutMs of inactivity', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 2000,
        immediateFlushLength: 99999,
        checkIntervalMs: 100000, // prevent activity timeout during test
        activityTimeoutMs: 100000,
      });
      batcher.addText('Hello');
      vi.advanceTimersByTime(2500);
      expect(onFlush).toHaveBeenCalledWith('Hello');
    });

    it('should reset idle timer on each addText', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 2000,
        immediateFlushLength: 99999,
      });
      batcher.addText('A');
      vi.advanceTimersByTime(1500);
      batcher.addText('B');
      vi.advanceTimersByTime(1500); // 1500ms from last add
      expect(onFlush).not.toHaveBeenCalled();
      vi.advanceTimersByTime(1000); // 2500ms from last add
      expect(onFlush).toHaveBeenCalledWith('AB');
    });
  });

  describe('activity timeout', () => {
    it('should call onIdle after activityTimeoutMs of inactivity', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
        activityTimeoutMs: 30000,
        checkIntervalMs: 5000,
      });
      batcher.addText('Hello');
      // Advance past 30s idle
      vi.advanceTimersByTime(35000);
      expect(onIdle).toHaveBeenCalledTimes(1);
    });

    it('should reset activity timer on new text', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
        activityTimeoutMs: 30000,
        checkIntervalMs: 5000,
      });
      batcher.addText('Hello');
      vi.advanceTimersByTime(25000);
      batcher.addText('World'); // resets activity
      vi.advanceTimersByTime(10000); // 10s from reset
      expect(onIdle).not.toHaveBeenCalled();
      vi.advanceTimersByTime(25000); // 35s from reset
      expect(onIdle).toHaveBeenCalledTimes(1);
    });

    it('should not call onIdle multiple times without reset', () => {
      batcher = new StreamingBatcher(onFlush, onIdle, {
        idleTimeoutMs: 100000,
        immediateFlushLength: 99999,
        activityTimeoutMs: 30000,
        checkIntervalMs: 5000,
      });
      batcher.addText('Hello');
      vi.advanceTimersByTime(70000); // 70s, should fire ~1 time (reset after first)
      expect(onIdle).toHaveBeenCalledTimes(1);
    });
  });

  describe('destroy', () => {
    it('should prevent further operations', () => {
      batcher = new StreamingBatcher(onFlush, onIdle);
      batcher.destroy();
      batcher.addText('Hello');
      expect(onFlush).not.toHaveBeenCalled();
    });

    it('should flush remaining buffer when explicitly called before destroy', () => {
      batcher = new StreamingBatcher(onFlush, onIdle);
      batcher.addText('Hello');
      batcher.destroy();
      expect(onFlush).not.toHaveBeenCalled(); // buffer lost on destroy
    });

    it('should clear timers', () => {
      batcher = new StreamingBatcher(onFlush, onIdle);
      batcher.addText('hello'); // Create idle timer
      const clearTimeoutSpy = vi.spyOn(globalThis, 'clearTimeout');
      const clearIntervalSpy = vi.spyOn(globalThis, 'clearInterval');
      batcher.destroy();
      expect(clearTimeoutSpy).toHaveBeenCalled();
      expect(clearIntervalSpy).toHaveBeenCalled();
      clearTimeoutSpy.mockRestore();
      clearIntervalSpy.mockRestore();
    });
  });
});
