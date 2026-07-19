/**
 * Claude Code session management: wraps the SDK's query() call with
 * message iteration callbacks for the WeChat adapter's main loop.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import { loadConfig } from './config.js';

export interface StartClaudeSessionOptions {
  cwd: string;
  prompt: string;
  resumeSessionId?: string;
  permissionMode?: 'default' | 'bypassPermissions';
  approvedTools?: string[];
  canUseTool?: (toolName: string, input: unknown) => Promise<'allow' | 'deny'>;
  onAssistantText?: (text: string) => void;
  onSessionInit?: (sessionId: string) => void;
  abortController: AbortController;
}

/**
 * Start a Claude Code session via the SDK's query() and process its message
 * stream, dispatching callbacks as each message type is encountered.
 *
 * Message iteration logic (§4.2):
 * - system/init → onSessionInit(sessionId)
 * - assistant/text → onAssistantText(text)
 * - assistant/tool_use → canUseTool(toolName, input)
 * - result → end
 *
 * Returns the session ID obtained from the init message and a success flag.
 */
export async function startClaudeSession(
  opts: StartClaudeSessionOptions,
): Promise<{ sessionId: string; success: boolean }> {
  const config = loadConfig();
  let sessionId = '';

  // Build env: spread process.env so the subprocess inherits PATH, HOME, etc.,
  // then overlay adapter-specific variables.
  const env: Record<string, string | undefined> = {
    ...(process.env as Record<string, string | undefined>),
    CLAUDE_CODE_ENTRYPOINT: 'remote_mobile',
  };
  if (config.httpProxy) {
    env.HTTP_PROXY = config.httpProxy;
  }
  if (config.httpsProxy) {
    env.HTTPS_PROXY = config.httpsProxy;
  }

  try {
    const gen = query({
      prompt: opts.prompt,
      options: {
        cwd: opts.cwd,
        abortController: opts.abortController,
        model: config.model,
        effort: config.effort,
        allowedTools: opts.approvedTools && opts.approvedTools.length > 0 ? opts.approvedTools : undefined,
        permissionMode: opts.permissionMode,
        resume: opts.resumeSessionId,
        env,
        // When bypassPermissions is requested the SDK requires this flag.
        allowDangerouslySkipPermissions:
          opts.permissionMode === 'bypassPermissions' ? true : undefined,
        ...(opts.canUseTool
          ? {
              canUseTool: async (toolName: string, input: Record<string, unknown>) => {
                const decision = await opts.canUseTool!(toolName, input);
                if (decision === 'allow') {
                  return { behavior: 'allow' as const, updatedInput: input };
                }
                return { behavior: 'deny' as const, message: 'User denied' };
              },
            }
          : {}),
      },
    });

    for await (const rawMsg of gen) {
      // Work with the message as a plain object to avoid pulling in
      // the full SDK type chain into this consumer module.
      const msg = rawMsg as Record<string, unknown>;

      if (msg.type === 'system' && msg.subtype === 'init') {
        const init = msg as { session_id: string };
        sessionId = init.session_id;
        opts.onSessionInit?.(sessionId);
        continue;
      }

      if (msg.type === 'assistant') {
        const assistant = msg as {
          message: { content: Array<Record<string, unknown>> };
        };
        for (const block of assistant.message.content) {
          if (block.type === 'text') {
            opts.onAssistantText?.(block.text as string);
          } else if (block.type === 'tool_use') {
            // canUseTool is handled by the SDK via options - no manual call needed here.
            continue;
          }
        }
        continue;
      }

      if (msg.type === 'result') {
        const result = msg as { subtype: string; session_id: string };
        return {
          sessionId: sessionId || result.session_id,
          success: result.subtype === 'success',
        };
      }
    }
  } catch (err: unknown) {
    // If the abort controller was triggered, return gracefully.
    if (opts.abortController.signal.aborted) {
      return { sessionId, success: false };
    }
    // Log and return gracefully for unexpected errors so the adapter loop
    // can continue processing other messages.
    console.error('Claude session error:', err);
    return { sessionId, success: false };
  }

  // The generator ended without a result message (shouldn't normally happen).
  return { sessionId, success: false };
}
