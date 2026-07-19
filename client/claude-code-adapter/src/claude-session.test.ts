import { describe, it, expect, vi, beforeEach, type MockInstance } from 'vitest';
import { startClaudeSession } from './claude-session.js';

// Mock the SDK before importing the module under test
vi.mock('@anthropic-ai/claude-agent-sdk', () => ({
  query: vi.fn(),
}));

vi.mock('./config.js', () => ({
  loadConfig: vi.fn(),
}));

import { query } from '@anthropic-ai/claude-agent-sdk';
import { loadConfig } from './config.js';
import type { Config } from './config.js';

// Helper: create mock SDK messages with minimal required fields
function systemInit(sessionId: string) {
  return {
    type: 'system',
    subtype: 'init',
    session_id: sessionId,
    cwd: '/tmp',
    tools: [],
    mcp_servers: [],
    model: 'sonnet',
    permissionMode: 'default',
    slash_commands: [],
    output_style: 'full',
    skills: [],
    plugins: [],
    claude_code_version: '2.1.214',
    apiKeySource: 'user',
    uuid: '00000000-0000-0000-0000-000000000001',
  } as const;
}

function assistantText(text: string, sessionId: string) {
  return {
    type: 'assistant',
    message: {
      content: [
        { type: 'text', text },
      ],
    },
    parent_tool_use_id: null,
    uuid: '00000000-0000-0000-0000-000000000002',
    session_id: sessionId,
  } as const;
}

function assistantToolUse(
  toolName: string,
  input: Record<string, unknown>,
  toolUseId: string,
  sessionId: string,
) {
  return {
    type: 'assistant',
    message: {
      content: [
        {
          type: 'tool_use',
          name: toolName,
          input,
          id: toolUseId,
        },
      ],
    },
    parent_tool_use_id: null,
    uuid: '00000000-0000-0000-0000-000000000003',
    session_id: sessionId,
  } as const;
}

function assistantMixed(text: string, toolName: string, toolInput: Record<string, unknown>, toolUseId: string, sessionId: string) {
  return {
    type: 'assistant',
    message: {
      content: [
        { type: 'text', text },
        {
          type: 'tool_use',
          name: toolName,
          input: toolInput,
          id: toolUseId,
        },
      ],
    },
    parent_tool_use_id: null,
    uuid: '00000000-0000-0000-0000-000000000004',
    session_id: sessionId,
  } as const;
}

function resultSuccess(sessionId: string) {
  return {
    type: 'result',
    subtype: 'success',
    is_error: false,
    result: 'ok',
    stop_reason: 'end_turn',
    num_turns: 1,
    total_cost_usd: 0,
    duration_ms: 100,
    duration_api_ms: 50,
    usage: { input_tokens: 10, output_tokens: 20, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
    modelUsage: {},
    permission_denials: [],
    uuid: '00000000-0000-0000-0000-000000000005',
    session_id: sessionId,
  } as const;
}

function resultError(error: string, sessionId: string) {
  return {
    type: 'result',
    subtype: 'error',
    is_error: true,
    result: 'error',
    stop_reason: null,
    num_turns: 0,
    total_cost_usd: 0,
    duration_ms: 100,
    duration_api_ms: 50,
    usage: { input_tokens: 10, output_tokens: 0, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
    modelUsage: {},
    permission_denials: [],
    error_message: error,
    uuid: '00000000-0000-0000-0000-000000000006',
    session_id: sessionId,
  } as const;
}

/**
 * Wraps a message sequence so that tool_use blocks invoke the SDK's
 * canUseTool wrapper from query options, simulating real SDK behavior.
 * @param sdkCanUseTool - the canUseTool function from query options
 */
async function* wrapWithCanUseTool(
  messages: unknown[],
  sdkCanUseTool?: (name: string, input: unknown) => Promise<{ behavior: string; updatedInput: unknown }>,
): AsyncGenerator<unknown> {
  for (const m of messages) {
    const record = m as Record<string, unknown>;
    if (record.type === 'assistant' && sdkCanUseTool) {
      const assistant = m as { message: { content: Array<Record<string, unknown>> } };
      for (const block of assistant.message.content) {
        if (block.type === 'tool_use') {
          await sdkCanUseTool(block.name as string, block.input);
        }
      }
    }
    yield m;
  }
}
async function* messageSequence(messages: unknown[]) {
  for (const m of messages) {
    yield m;
  }
}

function makeMockConfig(overrides: Partial<Config> = {}): Config {
  return {
    gatewayUrl: 'http://localhost:8765',
    agentName: 'claude',
    model: 'sonnet',
    cwd: '/tmp/test-cwd',
    pollIntervalMs: 50,
    effort: 'medium',
    sessionStorePath: '/tmp/test-sessions.json',
    ...overrides,
  };
}

describe('startClaudeSession', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(query).mockReset();
    vi.mocked(loadConfig).mockReturnValue(makeMockConfig());
  });

  it('should call onSessionInit on system/init message', async () => {
    const sessionId = 'session-abc-123';
    const gen = messageSequence([
      systemInit(sessionId),
      resultSuccess(sessionId),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    const onSessionInit = vi.fn();
    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'hello',
      abortController: new AbortController(),
      onSessionInit,
    });

    expect(onSessionInit).toHaveBeenCalledWith(sessionId);
    expect(result.sessionId).toBe(sessionId);
    expect(result.success).toBe(true);
  });

  it('should call onAssistantText for text content blocks', async () => {
    const sessionId = 'session-text-1';
    const gen = messageSequence([
      systemInit(sessionId),
      assistantText('Hello, world!', sessionId),
      assistantText('Next response.', sessionId),
      resultSuccess(sessionId),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    const onAssistantText = vi.fn();
    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'hello',
      abortController: new AbortController(),
      onAssistantText,
    });

    expect(onAssistantText).toHaveBeenCalledTimes(2);
    expect(onAssistantText).toHaveBeenNthCalledWith(1, 'Hello, world!');
    expect(onAssistantText).toHaveBeenNthCalledWith(2, 'Next response.');
    expect(result.success).toBe(true);
  });

  it('should call canUseTool and allow tool use', async () => {
    const sessionId = 'session-tool-1';

    vi.mocked(query).mockImplementation((opts: any) => {
      const canUse = opts.options?.canUseTool as ((name: string, input: unknown) => Promise<{ behavior: string; updatedInput: unknown }>) | undefined;
      const gen = wrapWithCanUseTool(
        [systemInit(sessionId), assistantToolUse('Bash', { command: 'ls' }, 'tool-1', sessionId), resultSuccess(sessionId)],
        canUse,
      );
      return gen as unknown as ReturnType<typeof query>;
    });

    const canUseTool = vi.fn().mockResolvedValue('allow' as const);
    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'run ls',
      abortController: new AbortController(),
      canUseTool,
    });

    expect(canUseTool).toHaveBeenCalledTimes(1);
    expect(canUseTool).toHaveBeenCalledWith('Bash', { command: 'ls' });
    expect(result.success).toBe(true);
  });

  it('should call canUseTool and deny tool use', async () => {
    const sessionId = 'session-tool-2';

    vi.mocked(query).mockImplementation((opts: any) => {
      const canUse = opts.options?.canUseTool as ((name: string, input: unknown) => Promise<{ behavior: string; updatedInput: unknown }>) | undefined;
      const gen = wrapWithCanUseTool(
        [systemInit(sessionId), assistantToolUse('Bash', { command: 'rm -rf /' }, 'tool-2', sessionId), resultSuccess(sessionId)],
        canUse,
      );
      return gen as unknown as ReturnType<typeof query>;
    });

    const canUseTool = vi.fn().mockResolvedValue('deny' as const);
    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'delete everything',
      abortController: new AbortController(),
      canUseTool,
    });

    expect(canUseTool).toHaveBeenCalledTimes(1);
    expect(canUseTool).toHaveBeenCalledWith('Bash', { command: 'rm -rf /' });
    expect(result.success).toBe(true);
  });

  it('should handle mixed text and tool_use in a single assistant message', async () => {
    const sessionId = 'session-mixed-1';

    vi.mocked(query).mockImplementation((opts: any) => {
      const canUse = opts.options?.canUseTool as ((name: string, input: unknown) => Promise<{ behavior: string; updatedInput: unknown }>) | undefined;
      const gen = wrapWithCanUseTool(
        [systemInit(sessionId), assistantMixed('Let me check...', 'Read', { path: '/tmp/test.txt' }, 'tool-3', sessionId), resultSuccess(sessionId)],
        canUse,
      );
      return gen as unknown as ReturnType<typeof query>;
    });

    const onAssistantText = vi.fn();
    const canUseTool = vi.fn().mockResolvedValue('allow' as const);
    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'read a file',
      abortController: new AbortController(),
      onAssistantText,
      canUseTool,
    });

    expect(onAssistantText).toHaveBeenCalledWith('Let me check...');
    expect(canUseTool).toHaveBeenCalledWith('Read', { path: '/tmp/test.txt' });
    expect(result.success).toBe(true);
  });

  it('should return success: false on error result', async () => {
    const sessionId = 'session-error-1';
    const gen = messageSequence([
      systemInit(sessionId),
      resultError('API rate limit exceeded', sessionId),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'hello',
      abortController: new AbortController(),
    });

    expect(result.success).toBe(false);
    expect(result.sessionId).toBe(sessionId);
  });

  it('should abort the loop when abortController is triggered', async () => {
    const sessionId = 'session-abort-1';
    const abortController = new AbortController();

    // Create a generator that responds to the abort signal,
    // matching how the real SDK stops yielding on abort.
    async function* abortAwareGen() {
      yield systemInit(sessionId);
      yield assistantText('thinking...', sessionId);

      // Wait for abort or an eventual fallback timeout.
      await new Promise<void>((resolve) => {
        const onAbort = () => resolve();
        abortController.signal.addEventListener('abort', onAbort, { once: true });
        // Fallback: resolve after a long time so the test doesn't hang
        // if something goes wrong.
        setTimeout(resolve, 5000);
      });

      // Never yield a result message after abort.
      if (!abortController.signal.aborted) {
        yield resultSuccess(sessionId);
      }
    }
    vi.mocked(query).mockReturnValue(abortAwareGen() as unknown as ReturnType<typeof query>);

    // Start the session and abort after a short delay
    const resultPromise = startClaudeSession({
      cwd: '/tmp',
      prompt: 'hello',
      abortController,
    });

    // Give enough time for systemInit to be processed
    await new Promise((r) => setTimeout(r, 10));
    abortController.abort();

    const result = await resultPromise;

    expect(result.success).toBe(false);
  });

  it('should preserve sessionId even on error', async () => {
    const sessionId = 'session-error-2';

    // Create a generator that throws after init
    async function* erroredGen() {
      yield systemInit(sessionId);
      throw new Error('Unexpected SDK error');
    }
    vi.mocked(query).mockReturnValue(erroredGen() as unknown as ReturnType<typeof query>);

    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'hello',
      abortController: new AbortController(),
    });

    expect(result.sessionId).toBe(sessionId);
    expect(result.success).toBe(false);
  });

  it('should set SDK env including CLAUDE_CODE_ENTRYPOINT and proxy vars', async () => {
    vi.mocked(loadConfig).mockReturnValue(makeMockConfig({
      httpProxy: 'http://proxy:8080',
      httpsProxy: 'https://proxy:8443',
      model: 'opus',
    }));

    const gen = messageSequence([
      systemInit('session-proxy-1'),
      resultSuccess('session-proxy-1'),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    await startClaudeSession({
      cwd: '/tmp',
      prompt: 'test',
      abortController: new AbortController(),
    });

    // Verify query was called with the right env
    const queryCall = vi.mocked(query).mock.calls[0][0];
    expect(queryCall.options?.env).toBeDefined();
    expect(queryCall.options!.env!['CLAUDE_CODE_ENTRYPOINT']).toBe('remote_mobile');
    expect(queryCall.options!.env!['HTTP_PROXY']).toBe('http://proxy:8080');
    expect(queryCall.options!.env!['HTTPS_PROXY']).toBe('https://proxy:8443');
    expect(queryCall.options!.model).toBe('opus');
  });

  it('should pass resumeSessionId and permissionMode to query options', async () => {
    const gen = messageSequence([
      systemInit('session-resume-1'),
      resultSuccess('session-resume-1'),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    await startClaudeSession({
      cwd: '/tmp',
      prompt: 'continue',
      resumeSessionId: 'prev-session-id',
      permissionMode: 'bypassPermissions',
      abortController: new AbortController(),
    });

    const queryCall = vi.mocked(query).mock.calls[0][0];
    expect(queryCall.options?.resume).toBe('prev-session-id');
    expect(queryCall.options?.permissionMode).toBe('bypassPermissions');
  });

  it('should pass approvedTools as allowedTools', async () => {
    const gen = messageSequence([
      systemInit('session-approve-1'),
      resultSuccess('session-approve-1'),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    await startClaudeSession({
      cwd: '/tmp',
      prompt: 'run',
      approvedTools: ['Bash', 'Read', 'Edit'],
      abortController: new AbortController(),
    });

    const queryCall = vi.mocked(query).mock.calls[0][0];
    expect(queryCall.options?.allowedTools).toEqual(['Bash', 'Read', 'Edit']);
  });

  it('should handle no canUseTool callback gracefully (skip tool_use)', async () => {
    const sessionId = 'session-no-can-1';
    const gen = messageSequence([
      systemInit(sessionId),
      assistantToolUse('Bash', { command: 'ls' }, 'tool-x', sessionId),
      resultSuccess(sessionId),
    ]);
    vi.mocked(query).mockReturnValue(gen as unknown as ReturnType<typeof query>);

    // No canUseTool provided — should not throw
    const result = await startClaudeSession({
      cwd: '/tmp',
      prompt: 'run ls',
      abortController: new AbortController(),
    });

    expect(result.success).toBe(true);
  });
});
