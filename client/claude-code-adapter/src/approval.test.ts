import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  parseApprovalCommand,
  createApprovalTimeout,
  formatApprovalPrompt,
} from './approval.js';

describe('parseApprovalCommand', () => {
  it.each([
    { text: '/approve', expected: 'approve' },
    { text: '/approve ', expected: 'approve' },
    { text: '/APPROVE', expected: 'approve' },
    { text: '  /approve  ', expected: 'approve' },
  ])('should parse "$text" as approve', ({ text, expected }) => {
    expect(parseApprovalCommand(text)?.type).toBe(expected);
  });

  it.each([
    { text: '/deny', expected: 'deny' },
    { text: '/deny ', expected: 'deny' },
    { text: '/DENY', expected: 'deny' },
  ])('should parse "$text" as deny', ({ text, expected }) => {
    expect(parseApprovalCommand(text)?.type).toBe(expected);
  });

  it.each([
    { text: '/approve session', expected: 'approve_session' },
    { text: '/approve  session', expected: 'approve_session' },
    { text: '/APPROVE SESSION', expected: 'approve_session' },
    { text: '  /approve session  ', expected: 'approve_session' },
  ])('should parse "$text" as approve_session', ({ text, expected }) => {
    expect(parseApprovalCommand(text)?.type).toBe(expected);
  });

  it.each([
    { text: '/approve on', expected: 'approve_on' },
    { text: '/approve  on', expected: 'approve_on' },
    { text: '/APPROVE ON', expected: 'approve_on' },
  ])('should parse "$text" as approve_on', ({ text, expected }) => {
    expect(parseApprovalCommand(text)?.type).toBe(expected);
  });

  it.each([
    { text: '/approve off', expected: 'approve_off' },
    { text: '/approve  off', expected: 'approve_off' },
    { text: '/APPROVE OFF', expected: 'approve_off' },
  ])('should parse "$text" as approve_off', ({ text, expected }) => {
    expect(parseApprovalCommand(text)?.type).toBe(expected);
  });

  it.each([
    { text: '/unknown' },
    { text: 'hello /approve' },
    { text: '' },
    { text: 'regular message' },
    { text: '/cd' },
    { text: '/use hermes' },
  ])('should return null for "$text"', ({ text }) => {
    expect(parseApprovalCommand(text)).toBeNull();
  });
});

describe('createApprovalTimeout', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('should resolve false after timeout', async () => {
    const promise = createApprovalTimeout(60000);
    vi.advanceTimersByTime(60000);
    const result = await promise;
    expect(result).toBe(false);
  });

  it('should not resolve before timeout', async () => {
    const promise = createApprovalTimeout(60000);
    vi.advanceTimersByTime(30000);

    // Use a resolved promise as signal — the timeout should NOT have resolved
    const result = await Promise.race([
      promise.then(() => 'resolved'),
      Promise.resolve('pending'),
    ]);
    expect(result).toBe('pending');

    vi.advanceTimersByTime(30000);
    const finalResult = await promise;
    expect(finalResult).toBe(false);
  });

  it('should work with custom timeout', async () => {
    const promise = createApprovalTimeout(1000);
    vi.advanceTimersByTime(1000);
    const result = await promise;
    expect(result).toBe(false);
  });
});

describe('formatApprovalPrompt', () => {
  it('should format Bash approval prompt', () => {
    const result = formatApprovalPrompt('wiki', 'Bash', { command: 'npm install' });
    expect(result).toContain('**claude**:wiki');
    expect(result).toContain('Bash');
    expect(result).toContain('npm install');
    expect(result).toContain('/approve');
    expect(result).toContain('/deny');
    expect(result).toContain('/approve session');
  });

  it('should format Read approval prompt', () => {
    const result = formatApprovalPrompt('gw', 'Read', { path: '/tmp/test.txt' });
    expect(result).toContain('**claude**:gw');
    expect(result).toContain('Read');
    expect(result).toContain('/tmp/test.txt');
  });
});
