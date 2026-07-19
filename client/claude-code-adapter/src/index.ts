/**
 * Claude Code Adapter — main entry point.
 *
 * Lifecycle: register with gateway → poll loop → message routing
 *   → session management → Claude SDK query → reply
 *
 * Features:
 * - T3.4: Streaming batching with idle timeout and buffer flush
 * - T3.5: 30s activity timeout sends "Claude is thinking..." prompt
 * - T3.6: Long replies (>3800 chars) split into numbered segments
 * - T3.7: Graceful shutdown via SIGINT/SIGTERM
 * - T3.8: Global error handlers (uncaughtException, unhandledRejection)
 * - T3.9: Poll loop with self-healing (retry on error)
 */

import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { type AgentMessage } from './gateway-client.js';
import { loadConfig } from './config.js';
import { GatewayClient } from './gateway-client.js';
import { QueryManager, type PendingApprovalState } from './query-manager.js';
import {
  loadAll,
  saveAll,
  type UserSessionData,
} from './session-store.js';
import { startClaudeSession } from './claude-session.js';
import {
  parseApprovalCommand,
  formatApprovalPrompt,
} from './approval.js';
import { formatStatus, switchCwd, addAlias, removeAlias, closeWorkspace } from './cd-command.js';
import { StreamingBatcher, splitLongReply } from './streaming-batcher.js';

const _require = createRequire(import.meta.url);

function getSdkVersion(): string {
  try {
    const pkgPath = _require.resolve('@anthropic-ai/claude-agent-sdk/package.json');
    const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf-8'));
    return pkg.version;
  } catch {
    return 'unknown';
  }
}

/**
 * Build a reply header with the Claude workspace prefix.
 */
function formatReplyHeader(basename: string): string {
  return `**claude**:${basename}\n---`;
}

export async function start(): Promise<void> {
  // T3.8: Global error handlers — catch unhandled rejections and exceptions
  // so the adapter loop always continues running.
  process.on('uncaughtException', (err) => {
    console.error('Uncaught exception:', err);
  });
  process.on('unhandledRejection', (reason) => {
    console.error('Unhandled rejection:', reason);
  });

  const config = loadConfig();
  const client = new GatewayClient(config.gatewayUrl, config.agentName);
  const queryManager = new QueryManager();
  const sessionData: Record<string, UserSessionData> = await loadAll(config.sessionStorePath);
  const permissionModeMap = new Map<string, 'default' | 'bypassPermissions'>();

  // Helper: persist session data to file
  function persistSessions(): void {
    saveAll(sessionData, config.sessionStorePath).catch((err) => {
      console.error('Failed to persist sessions:', err);
    });
  }

  console.log(`claude-code-adapter starting`);
  console.log(`Gateway URL: ${config.gatewayUrl}`);
  console.log(`Agent name: ${config.agentName}`);
  console.log(`SDK version: ${getSdkVersion()}`);
  console.log(`Model: ${config.model}`);
  console.log(`Effort: ${config.effort}`);
  console.log(`Poll interval: ${config.pollIntervalMs}ms`);

  // Set up shutdown signal first so we can interrupt registration retries
  let running = true;
  const onShutdown = () => {
    if (!running) return;
    running = false;
    console.log('Shutting down');
  };

  process.on('SIGINT', onShutdown);
  process.on('SIGTERM', onShutdown);

  // Register with retry on failure
  while (running) {
    try {
      const result = await client.register();
      console.log(`Registered: ${JSON.stringify(result)}`);
      break;
    } catch (err) {
      if (!running) break;
      console.warn(`Registration failed: ${err}`);
      await sleep(500);
    }
  }

  // If we were told to shut down during registration, exit immediately
  if (!running) return;

  /**
   * Ensure a user entry exists in sessionData. Creates a default entry
   * with empty aliases, empty sessions, and the configured cwd as activeCwd.
   */
  function ensureUser(wxid: string): UserSessionData {
    if (!sessionData[wxid]) {
      sessionData[wxid] = {
        aliases: {},
        activeCwd: config.cwd,
        sessions: {},
      };
    }
    return sessionData[wxid];
  }

  /**
   * Handle a single incoming message from the gateway.
   * Returns a promise that resolves once the message has been processed.
   */
  async function handleMessage(msg: AgentMessage): Promise<void> {
    const wxid = msg.from_user;
    const text = msg.text.trim();

    // ---- 1. Global command interception ----
    // /agent-help command
    if (/^\/agent-help\b/.test(text) || /^\/help\b/.test(text)) {
      const user = ensureUser(wxid);
      const basename = path.basename(user.activeCwd || config.cwd);
      await client.reply(msg.id, [
        formatReplyHeader(basename),
        '',
        '客户端命令:',
        '/cd             — 管理工作目录和别名',
        '/cd <target>    — 切换到指定目录或别名',
        '/cd + <n> <p>   — 添加别名',
        '/cd - <n>       — 删除别名',
        '/cd close <t>   — 关闭工作区',
        '/approve        — 批准当前工具调用',
        '/deny           — 拒绝当前工具调用',
        '/approve session— 批准并记住当前工具',
        '/approve on     — 开启自动审批模式',
        '/approve off    — 关闭自动审批模式',
        '/agent-help     — 显示此帮助',
        '/help           — 显示此帮助',
      ].join('\n'));
      return;
    }

    // /cd command
    if (/^\/cd\b/.test(text)) {
      const reply = handleCdCommand(wxid, text);
      if (reply) {
        await client.reply(msg.id, reply);
      }
      return;
    }

    const user = ensureUser(wxid);
    const cwd = user.activeCwd;

    // Skip if user has no activeCwd configured
    if (!cwd) {
      await client.reply(msg.id, [
        formatReplyHeader(path.basename(cwd || config.cwd)),
        '',
        '还没有工作目录，请先使用 /cd 命令切换到一个目录。',
      ].join('\n'));
      return;
    }

    const runningQuery = queryManager.get(wxid, cwd);

    // ---- 2. Approval commands ----
    const approvalCmd = parseApprovalCommand(text);
    if (approvalCmd) {
      const basename = path.basename(cwd);

      if (approvalCmd.type === 'approve_on') {
        permissionModeMap.set(wxid, 'bypassPermissions');
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '已切换为自动审批模式',
        ].join('\n'));
        return;
      }

      if (approvalCmd.type === 'approve_off') {
        permissionModeMap.set(wxid, 'default');
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '已切换为交互审批模式',
        ].join('\n'));
        return;
      }

      // Route approval/deny to pending approval
      if (runningQuery?.pendingApproval) {
        if (approvalCmd.type === 'approve') {
          runningQuery.pendingApproval.resolver(true);
          queryManager.clearPendingApproval(wxid, cwd);
        } else if (approvalCmd.type === 'deny') {
          runningQuery.pendingApproval.resolver(false);
          queryManager.clearPendingApproval(wxid, cwd);
        } else if (approvalCmd.type === 'approve_session') {
          // Add tool to session's approvedTools
          const toolName = runningQuery.pendingApproval.toolName;
          const sessionEntry = user.sessions[cwd];
          if (sessionEntry) {
            if (!sessionEntry.approvedTools.includes(toolName)) {
              sessionEntry.approvedTools.push(toolName);
            }
            persistSessions();
          }
          runningQuery.pendingApproval.resolver(true);
          queryManager.clearPendingApproval(wxid, cwd);
        }
      } else {
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '当前没有待审批的操作',
        ].join('\n'));
      }
      return;
    }

    // ---- 3. Routing: if cwd has a running query, queue the message ----
    if (runningQuery) {
      // Check if the query is actually still running by seeing if it has pending approval
      runningQuery.messageQueue.push({ id: msg.id, text });
      const basename = path.basename(cwd);
      await client.reply(msg.id, [
        formatReplyHeader(basename),
        '',
        '上一个请求仍在处理中，已加入队列',
      ].join('\n'));
      return;
    }

    // ---- 4. Start a new Claude session ----
    const basename = path.basename(cwd);

    // Ensure session entry exists
    if (!user.sessions[cwd]) {
      user.sessions[cwd] = {
        sessionId: null,
        lastActive: Date.now(),
        approvedTools: [],
      };
    }

    const sessionEntry = user.sessions[cwd];
    const newQuery = queryManager.start(wxid, cwd);
    const abortController = newQuery.abortController;

    // Create StreamingBatcher for this session (T3.4, T3.5, T3.6)
    const batcher = new StreamingBatcher(
      // onFlush: flush accumulated text to WeChat, splitting long messages
      (text) => {
        const segments = splitLongReply(formatReplyHeader(basename), text);
        for (let i = 0; i < segments.length; i++) {
          const segment = segments[i];
          if (i === 0) {
            // First segment gets the original reply
            client.reply(msg.id, segment).catch((err) => {
              console.error('Failed to flush reply:', err);
            });
          } else {
            // Continuation segments: delay 500ms between segments (T3.6)
            setTimeout(() => {
              client.reply(msg.id, segment).catch((err) => {
                console.error('Failed to send continuation:', err);
              });
            }, i * 500);
          }
        }
      },
      // onIdle: 30s inactivity notification (T3.5)
      () => {
        client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          'Claude 正在处理中...',
        ].join('\n')).catch((err) => {
          console.error('Failed to send idle notification:', err);
        });
      },
    );

    // Start Claude session (fire and forget — result is handled inside)
    startClaudeSession({
      cwd,
      prompt: text,
      resumeSessionId: sessionEntry.sessionId ?? undefined,
      permissionMode: permissionModeMap.get(wxid) ?? 'default',
      canUseTool: async (toolName, input) => {
        // Auto-approve when in bypassPermissions mode
        if (permissionModeMap.get(wxid) === 'bypassPermissions') {
          return 'allow' as const;
        }

        // Check session's approved tools
        if (sessionEntry.approvedTools.includes(toolName)) {
          return 'allow' as const;
        }

        // Flush buffer before showing approval prompt (T3.4 rule: tool_use → flush)
        batcher.toolUse();

        // Create a pending approval
        const approvalPromise = new Promise<boolean>((resolve) => {
          const timer = setTimeout(() => {
            // Auto-deny on timeout
            queryManager.clearPendingApproval(wxid, cwd);
            resolve(false);
          }, 60000);

          const approvalState: PendingApprovalState = {
            toolName,
            resolver: resolve,
            timer,
          };
          queryManager.setPendingApproval(wxid, cwd, approvalState);
        });

        // Notify user about pending approval with real tool input
        const approvalMsg = formatApprovalPrompt(basename, toolName, input);
        client.reply(msg.id, approvalMsg).catch((err) => {
          console.error('Failed to send approval prompt:', err);
        });

        const allowed = await approvalPromise;
        return allowed ? ('allow' as const) : ('deny' as const);
      },
      onAssistantText: (textBlock) => {
        batcher.addText(textBlock);
      },
      onSessionInit: (sessionId) => {
        sessionEntry.sessionId = sessionId;
        persistSessions();
      },
      abortController,
    })
      .then((result) => {
        // Flush remaining buffer on result (T3.4 rule: result → flush)
        batcher.flush();
        batcher.destroy();

        // Update lastActive
        sessionEntry.lastActive = Date.now();
        persistSessions();

        // Remove from query manager after completion, then process queued messages
        queryManager.remove(wxid, cwd);
        processQueue(wxid, cwd, newQuery.messageQueue);
      })
      .catch((err) => {
        console.error('Session error:', err);
        batcher.flush();
        batcher.destroy();

        // Still try to process queued messages after error
        queryManager.remove(wxid, cwd);
        processQueue(wxid, cwd, newQuery.messageQueue);
      });
  }

  /**
   * Process queued messages for a user's cwd.
   * Called after a session completes, one message at a time.
   */
  async function processQueue(
    wxid: string,
    cwd: string,
    queued: Array<{ id: string; text: string }>,
  ): Promise<void> {
    for (const item of queued) {
      if (!running) break;
      // Synthesize a minimal AgentMessage and pass to handleMessage
      const fakeMsg: AgentMessage = {
        id: item.id,
        from_user: wxid,
        text: item.text,
        timestamp: Date.now(),
        context_token: '',
        message_type: 'text',
        media: [],
      };
      await handleMessage(fakeMsg);
    }
  }

  /**
   * Handle /cd command for a user.
   */
  function handleCdCommand(wxid: string, text: string): string | null {
    const user = ensureUser(wxid);
    const parts = text.trim().split(/\s+/);

    // /cd — list status
    if (parts.length === 1) {
      return formatStatus(user);
    }

    const target = parts[1];

    // /cd + <alias> [<path>]
    if (target === '+') {
      const aliasName = parts[2];
      const aliasPath = parts[3];
      if (!aliasName) {
        return formatStatus(user);
      }
      return addAlias(user, aliasName, aliasPath, persistSessions);
    }

    // /cd - <alias>
    if (target === '-') {
      const aliasName = parts[2];
      if (!aliasName) {
        return formatStatus(user);
      }
      return removeAlias(user, aliasName, persistSessions);
    }

    // /cd close <target>
    if (target === 'close') {
      const closeTarget = parts[2];
      if (!closeTarget) {
        return formatStatus(user);
      }
      return closeWorkspace(user, closeTarget, {
        abort: (abortCwd) => {
          queryManager.abort(wxid, abortCwd);
        },
        save: persistSessions,
      });
    }

    // /cd <target> — switch workspace
    return switchCwd(user, target, persistSessions);
  }

  // Poll loop (T3.9: self-healing — poll errors are caught, logged, and retried on next interval)
  const poll = async () => {
    if (!running) return;
    try {
      const messages = await client.poll();
      for (const msg of messages) {
        console.log(`[${msg.from_user}] ${msg.text}`);
        try {
          await handleMessage(msg);
        } catch (err) {
          console.error(`Message handling error:`, err);
        }
      }
    } catch (err) {
      if (!running) return;
      // T3.9: Poll error is caught here — the poll loop simply skips this iteration
      // and will retry on the next interval. This provides automatic self-healing
      // for transient network issues or gateway restarts.
      console.warn(`Poll error: ${err}`);
    }
  };

  // Kick off the first poll immediately, then poll on interval
  const pollInterval = setInterval(() => { poll(); }, config.pollIntervalMs);
  poll();

  // Wait for shutdown signal
  return new Promise<void>((resolve) => {
    const shutdown = () => {
      if (!running) return;
      running = false;
      console.log('Shutting down...');

      // T3.7: Enhanced graceful shutdown
      // 1. Abort all running queries (resolves pending approvals as deny)
      queryManager.abortAll();
      // 2. Clear all timers
      clearInterval(pollInterval);
      process.removeListener('SIGINT', shutdown);
      process.removeListener('SIGTERM', shutdown);
      // 3. Give a short window for cleanup (flush logs etc.), then force exit
      setTimeout(() => {
        process.exit(0);
      }, 3000).unref();
      resolve();
    };

    // Replace the outer handler with one that also cleans up the interval
    process.removeListener('SIGINT', onShutdown);
    process.removeListener('SIGTERM', onShutdown);
    process.on('SIGINT', shutdown);
    process.on('SIGTERM', shutdown);
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

// Auto-start when run directly
if (
  process.argv[1] &&
  (process.argv[1].endsWith('/index.ts') || process.argv[1].endsWith('/index.js'))
) {
  start().catch((err) => {
    console.error('Fatal error:', err);
    process.exit(1);
  });
}
