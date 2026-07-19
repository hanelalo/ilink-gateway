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
 * Also handles long message splitting (> 2000 chars → multi-segment).
 * Splitting is markdown-aware: code blocks and tables are kept intact.
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

const FENCE_RE = /^```/;
const TABLE_ROW_RE = /^\s*\|/;

/**
 * Split text into markdown blocks, keeping code fences and table rows intact.
 *
 * - Blank lines separate blocks (paragraph boundaries)
 * - Code blocks (between ``` fences) are kept as single units
 * - Consecutive table rows (lines starting with `|`) are kept as single units
 */
function splitMarkdownBlocks(text: string): string[] {
  if (!text) return [];
  const blocks: string[] = [];
  const lines = text.split('\n');
  let current: string[] = [];
  let inCodeBlock = false;
  let inTable = false;

  const flush = () => {
    if (current.length > 0) {
      blocks.push(current.join('\n').trim());
      current = [];
    }
  };

  for (const rawLine of lines) {
    const trimmed = rawLine.trim();

    // Code fence toggles
    if (FENCE_RE.test(trimmed)) {
      if (!inCodeBlock) {
        flush();
        inTable = false;
      }
      current.push(rawLine);
      inCodeBlock = !inCodeBlock;
      if (!inCodeBlock) {
        flush();
      }
      continue;
    }

    if (inCodeBlock) {
      current.push(rawLine);
      continue;
    }

    // Blank line = block boundary
    if (!trimmed) {
      flush();
      inTable = false;
      continue;
    }

    // Table row - keep consecutive | lines together
    if (TABLE_ROW_RE.test(trimmed)) {
      if (!inTable) {
        flush();
        inTable = true;
      }
      current.push(rawLine);
      continue;
    }

    // Exiting table: non-table, non-empty line
    if (inTable) {
      flush();
      inTable = false;
    }

    // Regular text
    current.push(rawLine);
  }

  flush();
  return blocks;
}

/**
 * Greedily pack markdown blocks into segments under maxLen.
 * Single blocks exceeding maxLen are force-split at character boundary
 * with [N/M] prefix.
 */
function packBlocks(blocks: string[], maxLen: number): string[] {
  if (!blocks.length) return [];
  const packed: string[] = [];
  let current = '';
  const oversized: string[] = [];

  for (const block of blocks) {
    const candidate = current ? `${current}\n\n${block}` : block;
    if (candidate.length <= maxLen) {
      current = candidate;
      continue;
    }
    if (current) {
      packed.push(current);
      current = '';
    }
    if (block.length <= maxLen) {
      current = block;
      continue;
    }
    oversized.push(block);
  }

  if (current) packed.push(current);

  if (oversized.length > 0) {
    for (const block of oversized) {
      const totalSegments = packed.length + oversized.length;
      const segments = truncateBlock(block, maxLen, packed.length + 1, totalSegments);
      packed.push(...segments);
    }
  }

  return packed;
}

/**
 * Force-split a single oversized block at character boundary with [N/M] prefix.
 */
function truncateBlock(block: string, maxLen: number, startIndex: number, totalSegments: number): string[] {
  const result: string[] = [];
  let remaining = block;
  let segNum = startIndex;
  while (remaining.length > 0) {
    const prefix = `[${segNum}/${totalSegments}] `;
    const usable = maxLen - prefix.length;
    if (usable <= 0) {
      result.push(remaining.slice(0, maxLen));
      remaining = remaining.slice(maxLen);
    } else if (remaining.length <= usable) {
      result.push(`${prefix}${remaining}`);
      break;
    } else {
      result.push(`${prefix}${remaining.slice(0, usable)}`);
      remaining = remaining.slice(usable);
    }
    segNum++;
    totalSegments = Math.max(totalSegments, result.length);
  }
  return result;
}

/**
 * Split a message longer than `maxLen` into numbered segments.
 * Splitting is markdown-aware: code blocks and tables are kept intact.
 *
 * E.g. for oversized text:
 *   `[1/3] first-third`, `[2/3] second-third`, `[3/3] last-third`
 *
 * Segment content length is roughly balanced (maxLen per segment minus
 * overhead of the prefix like `[1/3] `).
 */
export function splitLongMessage(
  text: string,
  maxLen = 2000,
): string[] {
  if (!text) return [text];
  if (text.length <= maxLen) return [text];
  const blocks = splitMarkdownBlocks(text);
  return packBlocks(blocks, maxLen);
}

/**
 * Helper: split text into header + body and apply long-message splitting to body.
 * Returns arrays of full strings (each with the header prefixed once for the first segment
 * and a shorter "…" header for continuation segments).
 */
export function splitLongReply(
  header: string,
  body: string,
  maxLen = 2000,
): string[] {
  const headerLen = header.length + 2; // +2 for "\n\n"
  const bodyMaxLen = maxLen - headerLen;

  if (bodyMaxLen <= 0) {
    return [`${header}\n\n${body}`];
  }

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
