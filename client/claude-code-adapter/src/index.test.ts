import { describe, it, expect, vi, beforeEach, afterEach, type MockInstance } from 'vitest';
import { type Config } from './config.js';
import { type AgentMessage } from './gateway-client.js';

vi.mock('./config.js', () => ({
  loadConfig: vi.fn(),
}));

vi.mock('./gateway-client.js', () => ({
  GatewayClient: vi.fn(),
}));

vi.mock('./session-store.js', () => ({
  loadAll: vi.fn(),
  saveAll: vi.fn(),
}));

vi.mock('./claude-session.js', () => ({
  startClaudeSession: vi.fn(),
}));

import { loadConfig } from './config.js';
import { GatewayClient } from './gateway-client.js';
import { loadAll, saveAll } from './session-store.js';
import { startClaudeSession } from './claude-session.js';
import { QueryManager } from './query-manager.js';
import { start } from './index.js';

/**
 * Wait for a mock to be called at least `minCalls` times within `timeout` ms.
 */
async function waitForMock(
  spy: any,
  minCalls: number,
  timeout = 5000,
) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (spy.mock.calls.length >= minCalls) return;
    await new Promise((r) => setTimeout(r, 50));
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

function makeMessage(overrides: Partial<AgentMessage> = {}): AgentMessage {
  return {
    id: 'msg-1',
    from_user: 'wxid_test',
    text: 'hello',
    timestamp: 1000,
    context_token: 'ctx',
    message_type: 'text',
    media: [],
    ...overrides,
  };
}

describe('index start', () => {
  let mockPoll: ReturnType<typeof vi.fn>;
  let mockRegister: ReturnType<typeof vi.fn>;
  let mockReply: ReturnType<typeof vi.fn>;
  let mockSendProactive: ReturnType<typeof vi.fn>;
  let consoleLogSpy: MockInstance;
  let warnSpy: MockInstance;
  let abortAllSpy: MockInstance;

  beforeEach(() => {
    vi.clearAllMocks();

    mockPoll = vi.fn();
    mockRegister = vi.fn().mockResolvedValue({ ok: true, active_agent: 'claude' });
    mockReply = vi.fn().mockResolvedValue(true);
    mockSendProactive = vi.fn().mockResolvedValue(true);

    vi.mocked(loadConfig).mockReturnValue(makeMockConfig());
    vi.mocked(loadAll).mockResolvedValue({});
    vi.mocked(saveAll).mockResolvedValue();
    vi.mocked(startClaudeSession).mockResolvedValue({ sessionId: 'session-new', success: true });

    vi.mocked(GatewayClient).mockImplementation(() => {
      return {
        register: mockRegister,
        poll: mockPoll,
        reply: mockReply,
        sendProactive: mockSendProactive,
      } as unknown as GatewayClient;
    });

    consoleLogSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    abortAllSpy = vi.spyOn(QueryManager.prototype, 'abortAll');
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should load config, register, then poll for messages', async () => {
    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-1', from_user: 'wxid_test', text: 'hi there' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(consoleLogSpy, 9);

    expect(consoleLogSpy).toHaveBeenCalledWith(
      expect.stringContaining('Registered'),
    );
    expect(consoleLogSpy).toHaveBeenCalledWith('[wxid_test] hi there');
    expect(consoleLogSpy).toHaveBeenCalledWith(
      expect.stringContaining('SDK version'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;

    expect(abortAllSpy).toHaveBeenCalledTimes(1);
  });

  it('should handle registration failure with retry', async () => {
    mockRegister
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockResolvedValueOnce({ ok: true, active_agent: 'claude' });

    mockPoll.mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockRegister, 2);

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;

    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining('Connection refused'),
    );
    expect(mockRegister).toHaveBeenCalledTimes(2);
  });

  it('should log poll errors and continue', async () => {
    mockRegister.mockResolvedValue({ ok: true, active_agent: 'claude' });
    mockPoll
      .mockRejectedValueOnce(new Error('Poll failed'))
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(warnSpy, 1);

    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining('Poll failed'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should route /cd command and reply without starting Claude session', async () => {
    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-cd', from_user: 'wxid_test', text: '/cd' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-cd',
      expect.stringContaining('所有 workspace'),
      undefined,
      expect.stringContaining('agent'),
    );

    // Should NOT start a Claude session for a /cd command
    expect(startClaudeSession).not.toHaveBeenCalled();

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should route /cd <target> switch command', async () => {
    // First, create a session for the user
    vi.mocked(loadAll).mockResolvedValue({
      wxid_test: {
        aliases: { proj: '/tmp/project-a' },
        activeCwd: '/tmp/test-cwd',
        sessions: {
          '/tmp/project-a': {
            sessionId: 'session-uuid-abc',
            lastActive: Date.now(),
            approvedTools: [],
          },
          '/tmp/test-cwd': {
            sessionId: 'session-uuid-xyz',
            lastActive: Date.now(),
            approvedTools: [],
          },
        },
      },
    });

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-switch', from_user: 'wxid_test', text: '/cd proj' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-switch',
      expect.stringContaining('已切换到'),
      undefined,
      expect.stringContaining('agent'),
    );
    expect(mockReply).toHaveBeenCalledWith(
      'msg-switch',
      expect.stringContaining('project-a'),
      undefined,
      expect.stringContaining('agent'),
    );

    // /cd switches should persist sessions
    expect(saveAll).toHaveBeenCalled();

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should route /approve command when no pending approval', async () => {
    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-ap', from_user: 'wxid_test', text: '/approve' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-ap',
      expect.stringContaining('没有待审批的操作'),
      undefined,
      expect.stringContaining('agent'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should route /approve on and /approve off commands', async () => {
    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-on', from_user: 'wxid_test', text: '/approve on' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-on',
      expect.stringContaining('自动审批'),
      undefined,
      expect.stringContaining('agent'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should start Claude session for a regular message', async () => {
    // Mock startClaudeSession to simulate calling onAssistantText
    const mockStartSession = vi.mocked(startClaudeSession);
    mockStartSession.mockImplementation(
      ((opts: any) => {
        const onAssistantText = opts.onAssistantText as ((text: string) => void) | undefined;
        // Simulate Claude sending a reply
        onAssistantText?.('好的，我来帮你写一个脚本。');
        return Promise.resolve({ sessionId: 'session-new', success: true });
      }) as unknown as typeof startClaudeSession,
    );

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-regular', from_user: 'wxid_test', text: '帮我写个脚本' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    // Wait for reply (onAssistantText triggers flushReply which calls client.reply)
    await waitForMock(mockReply, 1);

    // Should have started a Claude session
    expect(startClaudeSession).toHaveBeenCalledTimes(1);
    const sessionOpts = vi.mocked(startClaudeSession).mock.calls[0][0];
    expect(sessionOpts.prompt).toBe('帮我写个脚本');
    expect(sessionOpts.cwd).toBe('/tmp/test-cwd');

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should queue message when cwd has a running query', async () => {
    // Use a never-resolving promise so the first session never completes
    const firstSessionPromise = new Promise<{ sessionId: string; success: boolean }>(() => {
      // Never resolve — keeps query "running"
    });

    const sessionInvocations: Array<{ prompt: string }> = [];
    vi.mocked(startClaudeSession).mockImplementation(
      ((opts: any) => {
        sessionInvocations.push({ prompt: opts.prompt as string });
        return firstSessionPromise;
      }) as unknown as typeof startClaudeSession,
    );

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-1', from_user: 'wxid_test', text: 'first message' }),
      ])
      // Second poll returns another message while first is still "running"
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-2', from_user: 'wxid_test', text: 'second message' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    // Wait for the first message to start a session
    await waitForMock(startClaudeSession, 1);

    // Now the first message is being processed — queryManager has an entry.
    // The second message should arrive in the next poll and find a running query.
    // Wait for reply to "msg-2" saying it's queued.
    await waitForMock(mockReply, 1, 10000);
    const queueReply = mockReply.mock.calls.find(
      (c: unknown[]) => (c[1] as string).includes('已加入队列'),
    );
    expect(queueReply).toBeDefined();
    expect(queueReply![0]).toBe('msg-2');

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should replay queued messages when session completes', async () => {
    // Control the first session's completion so we can queue msg-2 while it runs
    let resolveFirstSession!: (value: { sessionId: string; success: boolean }) => void;
    const firstSessionPromise = new Promise<{ sessionId: string; success: boolean }>(
      (resolve) => {
        resolveFirstSession = resolve;
      },
    );

    const sessionInvocations: Array<{ prompt: string }> = [];
    let callCount = 0;
    vi.mocked(startClaudeSession).mockImplementation(
      ((opts: any) => {
        callCount++;
        sessionInvocations.push({ prompt: opts.prompt as string });
        if (callCount === 1) {
          return firstSessionPromise;
        }
        // Subsequent calls resolve immediately
        return Promise.resolve({ sessionId: 'session-2', success: true });
      }) as unknown as typeof startClaudeSession,
    );

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-1', from_user: 'wxid_test', text: 'first message' }),
      ])
      // Second poll returns msg-2 while session is still running
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-2', from_user: 'wxid_test', text: 'second message' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    // Wait for msg-1 to start a session
    await waitForMock(startClaudeSession, 1);

    // Now msg-2 arrives and should be queued (reply 已加入队列)
    await waitForMock(mockReply, 1, 10000);

    // Resolve the first session — queued messages should now be replayed
    resolveFirstSession!({ sessionId: 'session-1', success: true });

    // Wait for the queued msg-2 to start a new session
    await waitForMock(startClaudeSession, 2, 10000);

    // Verify the second session started with the queued message (not the snapshot bug)
    expect(sessionInvocations).toHaveLength(2);
    expect(sessionInvocations[1].prompt).toBe('second message');

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should pass resumeSessionId from stored session on second message', async () => {
    // Setup: user already has a stored session for the active cwd
    const storedSessionId = 'uuid-prev-session';
    vi.mocked(loadAll).mockResolvedValue({
      wxid_test: {
        aliases: {},
        activeCwd: '/tmp/test-cwd',
        sessions: {
          '/tmp/test-cwd': {
            sessionId: storedSessionId,
            lastActive: Date.now() - 60000,
            approvedTools: ['Bash'],
          },
        },
      },
    });

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-resume', from_user: 'wxid_test', text: 'continue working' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(startClaudeSession, 1);

    const sessionOpts = vi.mocked(startClaudeSession).mock.calls[0][0];
    expect(sessionOpts.resumeSessionId).toBe(storedSessionId);

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should not pass resumeSessionId if no stored session exists', async () => {
    // User exists but has no session for the active cwd
    vi.mocked(loadAll).mockResolvedValue({
      wxid_test: {
        aliases: {},
        activeCwd: '/tmp/test-cwd',
        sessions: {}, // No session for /tmp/test-cwd
      },
    });

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-fresh', from_user: 'wxid_test', text: 'hello' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(startClaudeSession, 1);

    const sessionOpts = vi.mocked(startClaudeSession).mock.calls[0][0];
    expect(sessionOpts.resumeSessionId).toBeUndefined();

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should support parallel sessions in different cwds', async () => {
    // Make startClaudeSession hold onto the first session
    const firstSessionPromise = new Promise<{ sessionId: string; success: boolean }>(() => {
      // Never resolve — keeps query "running"
    });

    let sessionCallCount = 0;
    vi.mocked(startClaudeSession).mockImplementation(
      ((opts: any) => {
        sessionCallCount++;
        if (sessionCallCount === 1) {
          return firstSessionPromise;
        }
        return Promise.resolve({ sessionId: 'session-cd-2', success: true });
      }) as unknown as typeof startClaudeSession,
    );

    // User with existing sessions in two cwds
    vi.mocked(loadAll).mockResolvedValue({
      wxid_test: {
        aliases: { projA: '/tmp/project-a', projB: '/tmp/project-b' },
        activeCwd: '/tmp/project-a',
        sessions: {
          '/tmp/project-a': {
            sessionId: 'session-a1',
            lastActive: Date.now(),
            approvedTools: [],
          },
          '/tmp/project-b': {
            sessionId: 'session-b1',
            lastActive: Date.now(),
            approvedTools: [],
          },
        },
      },
    });

    mockPoll
      // First message to project-a
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-a-1', from_user: 'wxid_test', text: 'work on project a' }),
      ])
      // Second message: /cd to project-b, then message for project-b
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-switch-b', from_user: 'wxid_test', text: '/cd projB' }),
      ])
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-b-1', from_user: 'wxid_test', text: 'work on project b now' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    // Wait for first session to start (project-a)
    await waitForMock(startClaudeSession, 1);

    // Verify first session started with project-a prompt
    expect(vi.mocked(startClaudeSession).mock.calls[0][0].prompt).toBe('work on project a');

    // Wait for /cd switch to be processed
    await waitForMock(mockReply, 1);

    // Wait for second session to start (project-b) — this should work even though
    // project-a session is still running, since they're different cwds
    await waitForMock(startClaudeSession, 2, 10000);

    // Verify the second session started for project-b with its own prompt
    expect(sessionCallCount).toBe(2);
    expect(vi.mocked(startClaudeSession).mock.calls[1][0].prompt).toBe('work on project b now');

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should persist session ID on session init', async () => {
    const onSessionInitRef: { current?: (id: string) => void } = {};

    vi.mocked(startClaudeSession).mockImplementation(
      ((opts: any) => {
        onSessionInitRef.current = opts.onSessionInit as ((id: string) => void) | undefined;
        return Promise.resolve({ sessionId: 'session-init-id', success: true });
      }) as unknown as typeof startClaudeSession,
    );

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-init', from_user: 'wxid_test', text: 'hello' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(vi.mocked(startClaudeSession), 1);

    // Simulate the onSessionInit callback
    onSessionInitRef.current?.('session-uuid-123');

    // Wait for saveAll to be called after session init
    // This happens via the persistSessions call in startClaudeSession's onSessionInit
    await new Promise((r) => setTimeout(r, 100));

    expect(saveAll).toHaveBeenCalled();

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should route /deny command appropriately', async () => {
    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-deny', from_user: 'wxid_test', text: '/deny' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-deny',
      expect.stringContaining('没有待审批'),
      undefined,
      expect.stringContaining('agent'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should handle /cd switch with alias creation', async () => {
    vi.mocked(loadAll).mockResolvedValue({
      wxid_test: {
        aliases: {},
        activeCwd: '/tmp/test-cwd',
        sessions: {
          '/tmp/test-cwd': {
            sessionId: 'session-xyz',
            lastActive: Date.now(),
            approvedTools: [],
          },
        },
      },
    });

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-cd-alias', from_user: 'wxid_test', text: '/cd + myalias' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-cd-alias',
      expect.stringContaining('已添加别名'),
      undefined,
      expect.stringContaining('agent'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });

  it('should handle /cd close command', async () => {
    vi.mocked(startClaudeSession).mockImplementation(
      ((_opts: any) => {
        return Promise.resolve({ sessionId: 'session-close', success: true });
      }) as unknown as typeof startClaudeSession,
    );

    vi.mocked(loadAll).mockResolvedValue({
      wxid_test: {
        aliases: { target: '/tmp/target-proj' },
        activeCwd: '/tmp/test-cwd',
        sessions: {
          '/tmp/target-proj': {
            sessionId: 'session-close',
            lastActive: Date.now(),
            approvedTools: [],
          },
          '/tmp/test-cwd': {
            sessionId: 'session-other',
            lastActive: Date.now(),
            approvedTools: [],
          },
        },
      },
    });

    mockPoll
      .mockResolvedValueOnce([
        makeMessage({ id: 'msg-close', from_user: 'wxid_test', text: '/cd close target' }),
      ])
      .mockResolvedValue([]);

    const exitPromise = start();

    await waitForMock(mockReply, 1);

    expect(mockReply).toHaveBeenCalledWith(
      'msg-close',
      expect.stringContaining('已关闭 workspace'),
      undefined,
      expect.stringContaining('agent'),
    );

    process.emit('SIGINT', 'SIGINT');
    await exitPromise;
  });
});
