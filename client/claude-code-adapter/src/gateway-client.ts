export interface MediaItem {
  media_type: string;
  local_path: string;
  original_name?: string;
}

export interface AgentMessage {
  id: string;
  from_user: string;
  text: string;
  timestamp: number;
  context_token: string;
  message_type: string;
  media: MediaItem[];
  agent_context?: string;
}

export interface PollResponse {
  messages: AgentMessage[];
}

export interface RegisterResponse {
  ok: boolean;
  active_agent?: string;
  wechat_connected?: boolean;
  error?: string;
}

export interface ReplyResponse {
  ok: boolean;
}

export class GatewayClient {
  private baseUrl: string;
  private agentName: string;

  constructor(baseUrl: string, agentName: string) {
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.agentName = agentName;
  }

  /**
   * Wrap fetch with timeout and network error handling.
   * Returns `null` when the request fails (network error or timeout).
   */
  private async request(url: string, init?: RequestInit, timeoutMs: number = 10000): Promise<Response | null> {
    try {
      const res = await fetch(url, {
        ...init,
        signal: init?.signal ?? AbortSignal.timeout(timeoutMs),
      });
      return res;
    } catch {
      return null;
    }
  }

  async register(): Promise<RegisterResponse> {
    const res = await this.request(`${this.baseUrl}/api/agents/register`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: this.agentName, capabilities: ['text'] }),
    });
    if (!res) {
      return { ok: false, error: 'Network error or timeout' };
    }
    return res.json() as Promise<RegisterResponse>;
  }

  async poll(): Promise<AgentMessage[] | null> {
    const res = await this.request(
      `${this.baseUrl}/api/agents/${this.agentName}/poll`,
      undefined,
      30000,
    );
    if (!res) {
      return [];
    }
    if (res.status === 404) {
      return null; // agent not registered — caller should re-register
    }
    const data = (await res.json()) as PollResponse;
    return data.messages ?? [];
  }

  async reply(replyToId: string, text: string, mediaPaths?: string[], agentContext?: string): Promise<boolean> {
    const body: Record<string, unknown> = {
      reply_to_id: replyToId,
      text,
    };
    if (mediaPaths && mediaPaths.length > 0) {
      body.media_paths = mediaPaths;
    }
    if (agentContext) {
      body.agent_context = agentContext;
    }
    const res = await this.request(`${this.baseUrl}/api/agents/${this.agentName}/reply`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!res) return false;
    if (!res.ok) return false;
    const data = (await res.json()) as ReplyResponse;
    return data.ok === true;
  }

  async sendProactive(toUser: string, text: string): Promise<boolean> {
    const res = await this.request(`${this.baseUrl}/api/agents/${this.agentName}/reply`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        reply_to_id: '',
        text,
        to_user: toUser,
        context_token: '',
      }),
    });
    if (!res) return false;
    if (!res.ok) return false;
    const data = (await res.json()) as ReplyResponse;
    return data.ok === true;
  }
}
