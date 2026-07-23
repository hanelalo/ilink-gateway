/**
 * Claude Code Adapter - main entry point.
 *
 * Lifecycle: register with gateway → poll loop → message routing
 *   → session management → Claude SDK query → reply
 *
 * Features:
 * - T3.4: Streaming batching with idle timeout and buffer flush
 * - T3.5: 30s activity timeout sends "Claude is thinking..." prompt
 * - T3.6: Long replies (>2000 chars) split into numbered segments
 * - T3.7: Graceful shutdown via SIGINT/SIGTERM
 * - T3.8: Global error handlers (uncaughtException, unhandledRejection)
 * - T3.9: Poll loop with self-healing (retry on error)
 */

import os from 'node:os';
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
import {
  listSessionsForCwd,
  resolveSession,
  formatSessionList,
  formatSwitchReply,
} from './resume-command.js';
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
  // T3.8: Global error handlers - catch unhandled rejections and exceptions
  // so the adapter loop always continues running.
  process.on('uncaughtException', (err) => {
    console.error('Uncaught exception:', err);
  });
  process.on('unhandledRejection', (reason) => {
    console.error('Unhandled rejection:', reason);
  });

  // Clean up log files from previous calendar days (startup only)
  cleanupOldLogs();

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

    // ---- 0. 引用回复二次路由 ----
    if (msg.agent_context) {
      try {
        const ctx = JSON.parse(msg.agent_context);
        const targetWorkspace = ctx.workspace;
        if (targetWorkspace) {
          const user = ensureUser(wxid);
          const currentBasename = path.basename(user.activeCwd || config.cwd);
          if (targetWorkspace !== currentBasename) {
            // 尝试通过别名查找完整路径
            const aliasPath = user.aliases[targetWorkspace];
            if (aliasPath) {
              user.activeCwd = aliasPath;
              console.log(`[引用回复] 已切换 workspace: ${currentBasename} → ${targetWorkspace}`);
            } else {
              // 没有别名，遍历所有 session 的 cwd 找 basename 匹配的
              const match = Object.keys(user.sessions).find(
                key => path.basename(key) === targetWorkspace
              );
              if (match) {
                user.activeCwd = match;
                console.log(`[引用回复] 已切换 workspace: ${currentBasename} → ${targetWorkspace}`);
              } else {
                console.log(`[引用回复] 无法找到 workspace: ${targetWorkspace}，使用当前 workspace`);
              }
            }
          }
        }
      } catch {
        // agent_context 解析失败，忽略
      }
    }

    // ---- 1. Global command interception ----
    // /agent-help command
    if (/^\/agent-help\b/.test(text) || /^\/help\b/.test(text)) {
      const user = ensureUser(wxid);
      const basename = path.basename(user.activeCwd || config.cwd);
      const agentContext = JSON.stringify({ agent: config.agentName, workspace: basename });
      await client.reply(msg.id, [
        formatReplyHeader(basename),
        '',
        '客户端命令:',
        '/cd             - 管理工作目录和别名',
        '/cd <target>    - 切换到指定目录或别名',
        '/cd + <n> <p>   - 添加别名',
        '/cd - <n>       - 删除别名',
        '/close <target> - 关闭指定工作区',
        '/resume         - 列出当前 workspace 的历史 session',
        '/resume <id>    - 切换到指定 session（支持 UUID 前缀）',
        '/approve        - 批准当前工具调用',
        '/deny           - 拒绝当前工具调用',
        '/approve session- 批准并记住当前工具',
        '/approve on     - 开启自动审批模式',
        '/approve off    - 关闭自动审批模式',
        '/agent-help     - 显示此帮助',
        '/help           - 显示此帮助',
      ].join('\n'), undefined, agentContext);
      return;
    }

    // /close command - close workspace (top-level shortcut for /cd close)
    if (/^\/close\b/.test(text)) {
      const user = ensureUser(wxid);
      const parts = text.trim().split(/\s+/);
      const target = parts[1];
      if (!target) {
        const basename = path.basename(user.activeCwd || config.cwd);
        const agentContext = JSON.stringify({ agent: config.agentName, workspace: basename });
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '用法: /close <target>',
          '关闭指定工作区（终止运行中的 Claude session 并删除本地状态）',
        ].join('\n'), undefined, agentContext);
        return;
      }
      const reply = closeWorkspace(user, target, {
        abort: (abortCwd) => {
          queryManager.abort(wxid, abortCwd);
        },
        save: persistSessions,
      });
      // Extract workspace basename from the reply's header (e.g. "**claude**:foo")
      const matchHeader = reply.match(/\*\*claude\*\*:(\S+)/);
      const wsBasename = matchHeader ? matchHeader[1] : path.basename(user.activeCwd || config.cwd);
      const agentContext = JSON.stringify({ agent: config.agentName, workspace: wsBasename });
      await client.reply(msg.id, reply, undefined, agentContext);
      return;
    }

    // /cd command
    if (/^\/cd\b/.test(text)) {
      const reply = handleCdCommand(wxid, text);
      if (reply) {
        const cdBasename = path.basename(ensureUser(wxid).activeCwd || config.cwd);
        const agentContext = JSON.stringify({ agent: config.agentName, workspace: cdBasename });
        await client.reply(msg.id, reply, undefined, agentContext);
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
      ].join('\n'), undefined, JSON.stringify({ agent: config.agentName, workspace: path.basename(cwd || config.cwd) }));
      return;
    }

    // /resume command - list or switch Claude Code sessions in current cwd
    if (/^\/resume\b/.test(text)) {
      const basename = path.basename(cwd);
      const agentContext = JSON.stringify({ agent: config.agentName, workspace: basename });
      const parts = text.trim().split(/\s+/);
      const currentSessionId = user.sessions[cwd]?.sessionId ?? null;

      // /resume (no args) → list sessions
      if (parts.length === 1) {
        try {
          const sessions = await listSessionsForCwd(cwd);
          const reply = formatSessionList(sessions, currentSessionId, basename);
          await client.reply(msg.id, reply, undefined, agentContext);
        } catch (err) {
          console.error('listSessions error:', err);
          await client.reply(msg.id, [
            formatReplyHeader(basename),
            '',
            `读取 session 列表失败：${err instanceof Error ? err.message : String(err)}`,
          ].join('\n'), undefined, agentContext);
        }
        return;
      }

      // /resume <id> → switch session
      const inputId = parts.slice(1).join(' ');
      try {
        const sessions = await listSessionsForCwd(cwd);
        const result = resolveSession(inputId, sessions);
        if (result.error) {
          const lines = [formatReplyHeader(basename), '', result.error];
          if (result.matches && result.matches.length > 0) {
            lines.push('', '匹配的 session：');
            for (const m of result.matches) {
              lines.push(`  · [${m.sessionId.slice(0, 8)}] ${(m.customTitle || m.summary || m.firstPrompt || '(无标题)').slice(0, 40)}`);
            }
          }
          await client.reply(msg.id, lines.join('\n'), undefined, agentContext);
          return;
        }

        const session = result.session!;
        // Ensure session entry exists, then update sessionId
        if (!user.sessions[cwd]) {
          user.sessions[cwd] = {
            sessionId: null,
            lastActive: Date.now(),
            approvedTools: [],
          };
        }
        user.sessions[cwd].sessionId = session.sessionId;
        user.sessions[cwd].lastActive = Date.now();
        persistSessions();

        const reply = formatSwitchReply(basename, session);
        await client.reply(msg.id, reply, undefined, agentContext);
      } catch (err) {
        console.error('resume switch error:', err);
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          `切换 session 失败：${err instanceof Error ? err.message : String(err)}`,
        ].join('\n'), undefined, agentContext);
      }
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
        ].join('\n'), undefined, JSON.stringify({ agent: config.agentName, workspace: basename }));
        return;
      }

      if (approvalCmd.type === 'approve_off') {
        permissionModeMap.set(wxid, 'default');
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '已切换为交互审批模式',
        ].join('\n'), undefined, JSON.stringify({ agent: config.agentName, workspace: basename }));
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
        ].join('\n'), undefined, JSON.stringify({ agent: config.agentName, workspace: basename }));
      }
      return;
    }

    // ---- 2.5 /compact command: manually compact the session context ----
    if (/^\/compact\b/.test(text)) {
      const basename = path.basename(cwd);
      const agentContext = JSON.stringify({ agent: config.agentName, workspace: basename });
      const sessionId = user.sessions[cwd]?.sessionId;

      if (!sessionId) {
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '当前工作区还没有可压缩的会话历史，请先和 Claude 对话几轮。',
        ].join('\n'), undefined, agentContext);
        return;
      }

      // If a query is already running for this cwd, queue the command and
      // handle it once that query finishes (processQueue re-enters handleMessage).
      if (runningQuery) {
        runningQuery.messageQueue.push({ id: msg.id, text });
        await client.reply(msg.id, [
          formatReplyHeader(basename),
          '',
          '上一个请求仍在处理中，已加入队列',
        ].join('\n'), undefined, agentContext);
        return;
      }

      const compactQuery = queryManager.start(wxid, cwd);
      let compacted = false;
      let capturedText = '';
      startClaudeSession({
        cwd,
        prompt: text, // forward "/compact [instructions]" verbatim
        resumeSessionId: sessionId,
        permissionMode: permissionModeMap.get(wxid) ?? 'default',
        autoCompactWindow: config.autoCompactWindow,
        onAssistantText: (t) => { capturedText += t; },
        onCompact: (info) => {
          compacted = true;
          const lines = [
            formatReplyHeader(basename),
            '',
            '上下文已压缩（手动触发）',
            `压缩前 token 数：${info.preTokens}` +
              (info.postTokens != null ? `，压缩后约 ${info.postTokens}` : ''),
          ];
          client.reply(msg.id, lines.join('\n'), undefined, agentContext).catch((err) => {
            console.error('Failed to send compact reply:', err);
          });
        },
        abortController: compactQuery.abortController,
      })
        .then(() => {
          queryManager.remove(wxid, cwd);
          if (!compacted) {
            const body = capturedText.trim() || '压缩完成（无可用统计信息）。';
            client.reply(msg.id, [
              formatReplyHeader(basename),
              '',
              body,
            ].join('\n'), undefined, agentContext).catch((err) => {
              console.error('Failed to send compact reply:', err);
            });
          }
        })
        .catch((err) => {
          console.error('Compact error:', err);
          queryManager.remove(wxid, cwd);
        });
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
      ].join('\n'), undefined, JSON.stringify({ agent: config.agentName, workspace: basename }));
      return;
    }

    // ---- 4. Start a new Claude session ----
    const basename = path.basename(cwd);
    const agentContext = JSON.stringify({ agent: config.agentName, workspace: basename });

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
            client.reply(msg.id, segment, undefined, agentContext).catch((err) => {
              console.error('Failed to flush reply:', err);
            });
          } else {
            // Continuation segments: delay 500ms between segments (T3.6)
            setTimeout(() => {
              client.reply(msg.id, segment, undefined, agentContext).catch((err) => {
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
        ].join('\n'), undefined, agentContext).catch((err) => {
          console.error('Failed to send idle notification:', err);
        });
      },
    );

    // Start Claude session (fire and forget - result is handled inside)
    startClaudeSession({
      cwd,
      prompt: text,
      resumeSessionId: sessionEntry.sessionId ?? undefined,
      permissionMode: permissionModeMap.get(wxid) ?? 'default',
      autoCompactWindow: config.autoCompactWindow,
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
        client.reply(msg.id, approvalMsg, undefined, agentContext).catch((err) => {
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
      onCompact: (info) => {
        // Only notify for auto-compaction (manual /compact has its own reply
        // handled in the dedicated command branch above).
        if (info.trigger === 'manual') return;
        const lines = [
          formatReplyHeader(basename),
          '',
          '上下文已自动压缩',
          `压缩前 token 数：${info.preTokens}` +
            (info.postTokens != null ? `，压缩后约 ${info.postTokens}` : ''),
        ];
        client.reply(msg.id, lines.join('\n'), undefined, agentContext).catch((err) => {
          console.error('Failed to send auto-compact notice:', err);
        });
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
        agent_context: undefined,
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

    // /cd - list status
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

    // /cd <target> - switch workspace
    return switchCwd(user, target, persistSessions);
  }

  // Poll loop (T3.9: self-healing - poll errors are caught, logged, and retried on next interval)
  const poll = async () => {
    if (!running) return;
    try {
      const messages = await client.poll();
      // null means 404 - agent not registered, re-register
      if (messages === null) {
        console.warn('Agent not registered with gateway, re-registering...');
        try {
          const result = await client.register();
          console.log(`Re-registered: ${JSON.stringify(result)}`);
        } catch (err) {
          console.warn(`Re-registration failed: ${err}`);
        }
        return;
      }
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
      // T3.9: Poll error is caught here - the poll loop simply skips this iteration
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

/**
 * Remove log files in ~/.wechat-gateway/ that were last modified on a
 * previous calendar day. Runs once at startup, cross-platform.
 */
function cleanupOldLogs(): void {
  const homedir = os.homedir();
  const logDir = path.join(homedir, '.wechat-gateway');

  let entries: string[];
  try {
    entries = fs.readdirSync(logDir);
  } catch {
    return; // directory doesn't exist yet
  }

  const now = new Date();
  const today = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;

  for (const name of entries) {
    if (!name.endsWith('.log')) continue;
    const filePath = path.join(logDir, name);
    try {
      const stat = fs.statSync(filePath);
      if (!stat.isFile()) continue;
      const modTime = stat.mtime;
      const fileDay = `${modTime.getFullYear()}-${String(modTime.getMonth() + 1).padStart(2, '0')}-${String(modTime.getDate()).padStart(2, '0')}`;
      if (fileDay < today) {
        fs.unlinkSync(filePath);
        console.log(`Removed old log: ${name}`);
      }
    } catch {
      // skip files we can't stat or remove
    }
  }
}

// Auto-start when run directly (tsx, node, or compiled binary)
if (
  !process.argv[1] ||
  process.argv[1].endsWith('/index.ts') ||
  process.argv[1].endsWith('/index.js') ||
  process.argv[1].endsWith('wechat-claude')
) {
  start().catch((err) => {
    console.error('Fatal error:', err);
    process.exit(1);
  });
}
