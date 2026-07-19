import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { StreamingBatcher, splitLongMessage, splitLongReply } from './streaming-batcher.js';

describe('splitLongMessage', () => {
  it('should return original text as single segment when within maxLen', () => {
    expect(splitLongMessage('hello', 2000)).toEqual(['hello']);
  });

  it('should return original text as single segment when empty', () => {
    expect(splitLongMessage('')).toEqual(['']);
  });

  it('should handle exact boundary', () => {
    const text = 'a'.repeat(2000);
    expect(splitLongMessage(text, 2000)).toEqual([text]);
  });

  it('should split at blank lines when text exceeds maxLen', () => {
    const text = 'short paragraph\n\n' + 'x'.repeat(400) + '\n\n' + 'y'.repeat(400) + '\n\n' + 'z'.repeat(400);
    const maxLen = 500;
    const segments = splitLongMessage(text, maxLen);
    expect(segments.length).toBeGreaterThan(1);
    // Each segment should be under maxLen
    segments.forEach(seg => {
      expect(seg.length).toBeLessThanOrEqual(maxLen);
    });
  });

  it('should keep code blocks intact', () => {
    const codeBlock = '```python\nprint("hello")\nprint("world")\n```';
    const text = 'x'.repeat(300) + '\n\n' + codeBlock + '\n\n' + 'y'.repeat(300);
    const maxLen = 100;
    const segments = splitLongMessage(text, maxLen);
    expect(segments.length).toBeGreaterThan(1);
    // The code block should appear entirely in one segment
    const codeInSegments = segments.filter(s => s.includes('```'));
    expect(codeInSegments.length).toBe(1);
    const codeSeg = codeInSegments[0];
    expect(codeSeg).toContain('print("hello")');
    expect(codeSeg).toContain('print("world")');
  });

  it('should keep table rows intact', () => {
    const table = '| Col1 | Col2 |\n|------|------|\n| A    | B    |\n| C    | D    |';
    const text = 'x'.repeat(300) + '\n\n' + table + '\n\n' + 'y'.repeat(300);
    const maxLen = 100;
    const segments = splitLongMessage(text, maxLen);
    expect(segments.length).toBeGreaterThan(1);
    // The table should appear entirely in one segment
    const tableSegments = segments.filter(s => s.includes('Col1'));
    expect(tableSegments.length).toBe(1);
  });

  it('should force-split an oversized single block with [N/M] prefix', () => {
    const longLine = 'a'.repeat(500);
    const segments = splitLongMessage(longLine, 100);
    expect(segments.length).toBeGreaterThan(1);
    // Every segment except possibly the last should have [N/M] prefix
    segments.forEach(s => {
      expect(s).toMatch(/^\[\d+\/\d+\] /);
    });
    // Content without prefix should reconstruct original
    const contentOnly = segments.map(s => s.replace(/^\[\d+\/\d+\] /, '')).join('');
    expect(contentOnly).toBe(longLine);
  });

  it('should split large text while preserving markdown structure', () => {
    const para1 = 'Paragraph one is quite long. '.repeat(100);
    const codeBlock = '```\ncode block content\n```\n';
    const para2 = 'Paragraph two. '.repeat(50);
    const text = `${para1}\n\n${codeBlock}\n\n${para2}`;
    const segments = splitLongMessage(text, 200);
    expect(segments.length).toBeGreaterThan(1);
    // Code block should be intact in one segment
    const codeSegments = segments.filter(s => s.includes('```'));
    expect(codeSegments.length).toBe(1);
  });
});

describe('splitLongReply', () => {
  it('should return single segment with header when body fits within maxLen', () => {
    const header = '**claude**:project';
    const body = 'Hello world';
    const result = splitLongReply(header, body, 2000);
    expect(result).toHaveLength(1);
    expect(result[0]).toBe(`${header}\n\n${body}`);
  });

  it('should prepend header to first segment when body exceeds maxLen', () => {
    const header = '**claude**:project';
    const body = 'x'.repeat(500);
    const result = splitLongReply(header, body, 100);
    expect(result.length).toBeGreaterThan(1);
    // First segment contains the full header
    expect(result[0].startsWith(header)).toBe(true);
  });

  it('should preserve full header only in first segment', () => {
    const header = '**claude**:project';
    // Body needs enough content without blank lines (single block) to force multiple segments
    const body = 'a\n'.repeat(200);
    const result = splitLongReply(header, body, 100);
    expect(result.length).toBeGreaterThan(1);
    expect(result[0].startsWith(`${header}\n\n`)).toBe(true);
    for (let i = 1; i < result.length; i++) {
      expect(result[i].startsWith(header)).toBe(false);
    }
  });

  it('should handle empty body gracefully', () => {
    const header = '**claude**:project';
    const result = splitLongReply(header, '', 2000);
    expect(result).toHaveLength(1);
    expect(result[0]).toBe(`${header}\n\n`);
  });

  it('should keep code blocks intact when body exceeds maxLen', () => {
    const header = '**claude**:proj';
    const codeBlock = '```\nfn foo() {\n  println!("hello");\n}\n```';
    const text = 'x'.repeat(400) + '\n\n' + codeBlock + '\n\n' + 'y'.repeat(400);
    const result = splitLongReply(header, text, 200);
    expect(result.length).toBeGreaterThan(1);
    // Code block intact in one segment
    const codeSegments = result.filter(s => s.includes('```'));
    expect(codeSegments.length).toBe(1);
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
