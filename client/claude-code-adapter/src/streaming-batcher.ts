/**
 * Streaming batcher for batching assistant text and managing timeouts.
 *
 * Implements design doc §4.7 streaming batching rules:
 * - onAssistantText: cache text in buffer, flush when > 1500 chars
 * - tool_use: flush buffer then send tool notification
 * - result: flush remaining buffer
 * - 2s idle: flush buffer if no new text
 * - 30s activity: notify "Claude is thinking..." (timeout prompt)
 *
 * Also handles T3.6 long message splitting (> 3800 chars → multi-segment).
 */

export interface StreamingBatcherOpts {
  /** Buffer chars that trigger an immediate flush (default 1500) */
  immediateFlushLength?: number;
  /** ms of inactivity before idle flush (default 2000) */
  idleTimeoutMs?: number;
  /** ms of no activity before calling onIdle (default 30000) */
  activityTimeoutMs?: number;
  /** How often to check activity timeout (default 5000) */
  checkIntervalMs?: number;
}

/**
 * Split a message longer than `maxLen` into numbered segments.
 * E.g. `[1/3] first-third`, `[2/3] second-third`, `[3/3] last-third`
 *
 * Segment content length is roughly balanced (maxLen per segment minus
 * overhead of the prefix like `[1/3] `).
 */
export function splitLongMessage(
  text: string,
  maxLen = 3800,
): string[] {
  if (text.length <= maxLen) return [text];

  // Calculate number of segments
  // Reserve ~10 chars per segment for "[N/M] " prefix
  const prefixOverhead = 10;
  const usableLen = maxLen - prefixOverhead;

  if (usableLen <= 0) {
    // Extreme case: maxLen is too small for prefix, just return as-is
    return [text];
  }

  // Try to split at a reasonable boundary
  const segments: string[] = [];
  let remaining = text;

  while (remaining.length > 0) {
    // Calculate total segments for prefix
    const totalSegments = Math.ceil(text.length / usableLen);
    const currentSegment = segments.length + 1;

    // Take a chunk
    let chunk = remaining.slice(0, usableLen);
    remaining = remaining.slice(usableLen);

    // Add prefix
    segments.push(`[${currentSegment}/${totalSegments}] ${chunk}`);
  }

  return segments;
}

/**
 * Helper: split text into header + body and apply long-message splitting to body.
 * Returns arrays of full strings (each with the header prefixed once for the first segment
 * and a shorter "…" header for continuation segments).
 */
export function splitLongReply(
  header: string,
  body: string,
  maxLen = 3800,
): string[] {
  const headerLen = header.length + 2; // +2 for "\n\n"
  const bodyMaxLen = maxLen - headerLen;

  if (body.length <= bodyMaxLen) {
    return [`${header}\n\n${body}`];
  }

  const segments = splitLongMessage(body, bodyMaxLen);
  if (segments.length <= 1) {
    return [`${header}\n\n${body}`];
  }

  // First segment gets the full header
  segments[0] = `${header}\n\n${segments[0]}`;
  return segments;
}

/**
 * Streaming batcher that accumulates assistant text, flushing on:
 * 1. Buffer exceeds immediateFlushLength chars
 * 2. tool_use notification
 * 3. Explicit flush() call (on result)
 * 4. Idle timeout (no new text for idleTimeoutMs)
 *
 * Also fires onIdle callback when no activity for activityTimeoutMs
 * (for the "Claude is thinking..." prompt).
 */
export class StreamingBatcher {
  private buffer = '';
  private destroyed = false;

  private idleTimer: ReturnType<typeof setTimeout> | null = null;
  private activityTimer: ReturnType<typeof setInterval> | null = null;
  private lastActivity = 0;
  private idleFired = false;

  private readonly immediateFlushLength: number;
  private readonly idleTimeoutMs: number;
  private readonly activityTimeoutMs: number;
  private readonly checkIntervalMs: number;

  constructor(
    private readonly onFlush: (text: string) => void,
    private readonly onIdle: () => void,
    opts?: StreamingBatcherOpts,
  ) {
    this.immediateFlushLength = opts?.immediateFlushLength ?? 1500;
    this.idleTimeoutMs = opts?.idleTimeoutMs ?? 2000;
    this.activityTimeoutMs = opts?.activityTimeoutMs ?? 30000;
    this.checkIntervalMs = opts?.checkIntervalMs ?? 5000;

    this.lastActivity = Date.now();

    // Start periodic activity check
    this.activityTimer = setInterval(() => {
      this.checkActivity();
    }, this.checkIntervalMs);
  }

  /**
   * Add text to the buffer. Resets idle timer.
   * Flushes immediately if buffer exceeds immediateFlushLength.
   */
  addText(text: string): void {
    if (this.destroyed) return;
    this.buffer += text;
    this.resetIdleTimer();
    this.resetActivity();

    if (this.buffer.length > this.immediateFlushLength) {
      this.doFlush();
    }
  }

  /**
   * Called when a tool_use is encountered. Flushes buffer immediately.
   */
  toolUse(): void {
    if (this.destroyed) return;
    this.doFlush();
    this.resetActivity();
  }

  /**
   * Explicit flush (e.g. on result). Flushes buffer.
   */
  flush(): void {
    if (this.destroyed) return;
    this.doFlush();
  }

  /**
   * Destroy the batcher, clearing all timers.
   */
  destroy(): void {
    this.destroyed = true;
    this.clearTimers();
  }

  private doFlush(): void {
    if (!this.buffer) return;
    this.onFlush(this.buffer);
    this.buffer = '';
    this.resetIdleTimer();
  }

  private resetIdleTimer(): void {
    if (this.idleTimer) {
      clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    this.idleTimer = setTimeout(() => {
      this.doFlush();
    }, this.idleTimeoutMs);
  }

  private resetActivity(): void {
    this.lastActivity = Date.now();
    this.idleFired = false;
  }

  private checkActivity(): void {
    if (this.destroyed) return;
    const elapsed = Date.now() - this.lastActivity;
    if (elapsed >= this.activityTimeoutMs && !this.idleFired) {
      this.idleFired = true;
      this.onIdle();
    }
  }

  private clearTimers(): void {
    if (this.idleTimer) {
      clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    if (this.activityTimer) {
      clearInterval(this.activityTimer);
      this.activityTimer = null;
    }
  }
}
