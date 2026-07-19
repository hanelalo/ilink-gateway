/**
 * Query manager: tracks running Claude Code sessions keyed by (wxid, cwd).
 *
 * Each running session has an AbortController, optional pending-approval state,
 * a message queue for streaming text blocks, and a reply buffer for accumulating
 * assistant output before dispatch.
 */

/** A single pending tool approval for a running session. */
export interface PendingApprovalState {
  toolName: string;
  resolver: (allow: boolean) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** A single running Claude Code session for a given user + workspace pair. */
export interface RunningQuery {
  abortController: AbortController;
  pendingApproval: PendingApprovalState | null;
  messageQueue: Array<{ id: string; text: string }>;
}

type WxidMap = Map<string, RunningQuery>;

/**
 * Manages a 2-level map: wxid → cwd → RunningQuery.
 *
 * All methods are synchronous - the map is pure in-memory state.
 */
export class QueryManager {
  /** wxid → { cwd → RunningQuery } */
  private store = new Map<string, WxidMap>();

  /**
   * Start (or restart) a session for the given wxid and cwd.
   * If a session already exists for this pair, it is aborted first.
   * Returns the new RunningQuery.
   */
  start(wxid: string, cwd: string): RunningQuery {
    // Abort existing query for the same pair
    const existing = this.get(wxid, cwd);
    if (existing) {
      const prev = existing.pendingApproval;
      if (prev) {
        clearTimeout(prev.timer);
      }
      existing.abortController.abort();
    }

    let cwdMap = this.store.get(wxid);
    if (!cwdMap) {
      cwdMap = new Map();
      this.store.set(wxid, cwdMap);
    }

    const query: RunningQuery = {
      abortController: new AbortController(),
      pendingApproval: null,
      messageQueue: [],
    };

    cwdMap.set(cwd, query);
    return query;
  }

  /**
   * Get a running session for the given wxid and cwd.
   * Returns undefined when no session exists.
   */
  get(wxid: string, cwd: string): RunningQuery | undefined {
    return this.store.get(wxid)?.get(cwd);
  }

  /**
   * Remove and return a running session without aborting it.
   * Returns true if a session was removed, false otherwise.
   */
  remove(wxid: string, cwd: string): boolean {
    const query = this.get(wxid, cwd);
    if (!query) return false;

    const prev = query.pendingApproval;
    if (prev) {
      clearTimeout(prev.timer);
    }
    query.pendingApproval = null;

    const cwdMap = this.store.get(wxid)!;
    const removed = cwdMap.delete(cwd);

    // Clean up empty outer maps
    if (cwdMap.size === 0) {
      this.store.delete(wxid);
    }

    return removed;
  }

  /**
   * Abort a running session and remove it from the store.
   * Returns true if a session was aborted, false otherwise.
   */
  abort(wxid: string, cwd: string): boolean {
    const query = this.get(wxid, cwd);
    if (!query) return false;

    query.abortController.abort();
    this.remove(wxid, cwd);
    return true;
  }

  /**
   * Set the pending approval state for an existing session.
   * Clears the previous timer if replacing an existing approval.
   * No-op when no session exists for the given pair.
   */
  setPendingApproval(
    wxid: string,
    cwd: string,
    approval: PendingApprovalState,
  ): void {
    const query = this.get(wxid, cwd);
    if (!query) return;

    const prev = query.pendingApproval;
    if (prev) {
      clearTimeout(prev.timer);
    }

    query.pendingApproval = approval;
  }

  /**
   * Abort all running queries across all users and workspaces.
   * Clears all pending approvals with a deny resolution.
   * Used during graceful shutdown (T3.7).
   */
  abortAll(): void {
    for (const [wxid, cwdMap] of this.store) {
      for (const [cwd, query] of cwdMap) {
        const prev = query.pendingApproval;
        if (prev) {
          clearTimeout(prev.timer);
          prev.resolver(false);
        }
        query.abortController.abort();
      }
    }
    this.store.clear();
  }

  /**
   * Clear the pending approval state for an existing session.
   * No-op when no session exists or no approval is pending.
   */
  clearPendingApproval(wxid: string, cwd: string): void {
    const query = this.get(wxid, cwd);
    if (!query) return;

    const prev = query.pendingApproval;
    if (prev) {
      clearTimeout(prev.timer);
    }

    query.pendingApproval = null;
  }
}
