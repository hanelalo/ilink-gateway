import { describe, it, expect, beforeEach, vi } from 'vitest';
import { GatewayClient } from './gateway-client.js';

const BASE = 'http://localhost:8765';

function mockFetch(status: number, body: unknown, statusText?: string) {
  return vi.mocked(fetch).mockResolvedValueOnce(
    new Response(JSON.stringify(body), {
      status,
      statusText,
      headers: { 'content-type': 'application/json' },
    }),
  );
}

beforeEach(() => {
  vi.spyOn(globalThis, 'fetch').mockReset();
});

describe('GatewayClient', () => {
  describe('register', () => {
    it('should POST to /api/agents/register with name and capabilities', async () => {
      mockFetch(200, { ok: true, active_agent: 'claude', wechat_connected: false });

      const client = new GatewayClient(BASE, 'claude');
      const res = await client.register();

      expect(fetch).toHaveBeenCalledWith(
        `${BASE}/api/agents/register`,
        expect.objectContaining({
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ name: 'claude', capabilities: ['text'] }),
        }),
      );
      expect(res.ok).toBe(true);
      expect(res.active_agent).toBe('claude');
    });

    it('should return error response on 400', async () => {
      mockFetch(400, { ok: false, error: 'Agent already registered' });

      const client = new GatewayClient(BASE, 'claude');
      const res = await client.register();
      expect(res.ok).toBe(false);
      expect(res.error).toBe('Agent already registered');
    });
  });

  describe('poll', () => {
    it('should return messages from poll endpoint', async () => {
      const messages = [
        { id: 'msg-1', from_user: 'wxid_abc', text: 'hello', timestamp: 1000, context_token: 'ctx', message_type: 'text', media: [] },
      ];
      mockFetch(200, { messages });

      const client = new GatewayClient(BASE, 'claude');
      const result = await client.poll();
      expect(result).toEqual(messages);
    });

    it('should return empty array on 404', async () => {
      mockFetch(404, { error: 'Agent not found' });

      const client = new GatewayClient(BASE, 'claude');
      const result = await client.poll();
      expect(result).toEqual([]);
    });

    it('should return empty array when messages field is missing', async () => {
      mockFetch(200, {});

      const client = new GatewayClient(BASE, 'claude');
      const result = await client.poll();
      expect(result).toEqual([]);
    });
  });

  describe('reply', () => {
    it('should POST reply and return ok', async () => {
      mockFetch(200, { ok: true });

      const client = new GatewayClient(BASE, 'claude');
      const ok = await client.reply('msg-1', 'hello back');
      expect(ok).toBe(true);
      expect(fetch).toHaveBeenCalledWith(
        `${BASE}/api/agents/claude/reply`,
        expect.objectContaining({
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ reply_to_id: 'msg-1', text: 'hello back' }),
        }),
      );
    });

    it('should include media_paths when provided', async () => {
      mockFetch(200, { ok: true });

      const client = new GatewayClient(BASE, 'claude');
      await client.reply('msg-1', 'with file', ['/tmp/file.pdf']);
      const callArgs = vi.mocked(fetch).mock.calls[0][1] as RequestInit;
      const body = JSON.parse(callArgs.body as string);
      expect(body.media_paths).toEqual(['/tmp/file.pdf']);
    });

    it('should return false on non-200', async () => {
      mockFetch(500, {});

      const client = new GatewayClient(BASE, 'claude');
      const ok = await client.reply('msg-1', 'hello');
      expect(ok).toBe(false);
    });
  });

  describe('sendProactive', () => {
    it('should POST reply with to_user for proactive send', async () => {
      mockFetch(200, { ok: true });

      const client = new GatewayClient(BASE, 'claude');
      const ok = await client.sendProactive('wxid_abc', 'notification');
      expect(ok).toBe(true);
      expect(fetch).toHaveBeenCalledWith(
        `${BASE}/api/agents/claude/reply`,
        expect.objectContaining({
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            reply_to_id: '',
            text: 'notification',
            to_user: 'wxid_abc',
            context_token: '',
          }),
        }),
      );
    });
  });

  describe('network errors', () => {
    it('should return error object on register network failure', async () => {
      vi.mocked(fetch).mockRejectedValueOnce(new TypeError('Failed to fetch'));

      const client = new GatewayClient(BASE, 'claude');
      const res = await client.register();

      expect(res.ok).toBe(false);
      expect(res.error).toBeDefined();
    });

    it('should return empty array on poll network failure', async () => {
      vi.mocked(fetch).mockRejectedValueOnce(new TypeError('Failed to fetch'));

      const client = new GatewayClient(BASE, 'claude');
      const result = await client.poll();

      expect(result).toEqual([]);
    });

    it('should return false on reply network failure', async () => {
      vi.mocked(fetch).mockRejectedValueOnce(new TypeError('Failed to fetch'));

      const client = new GatewayClient(BASE, 'claude');
      const ok = await client.reply('msg-1', 'hello');

      expect(ok).toBe(false);
    });

    it('should return false on sendProactive network failure', async () => {
      vi.mocked(fetch).mockRejectedValueOnce(new TypeError('Failed to fetch'));

      const client = new GatewayClient(BASE, 'claude');
      const ok = await client.sendProactive('wxid_abc', 'notification');

      expect(ok).toBe(false);
    });
  });
});
