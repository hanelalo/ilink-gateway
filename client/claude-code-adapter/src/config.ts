import os from 'node:os';

export interface Config {
  gatewayUrl: string;
  agentName: string;
  model: string;
  cwd: string;
  pollIntervalMs: number;
  effort: 'low' | 'medium' | 'high' | 'xhigh' | 'max';
  sessionStorePath: string;
  autoCompactWindow?: number;
  httpProxy?: string;
  httpsProxy?: string;
}

export function loadConfig(): Config {
  return {
    gatewayUrl: envStr('CLAUDE_GATEWAY_URL', 'http://127.0.0.1:8765'),
    agentName: envStr('CLAUDE_GATEWAY_AGENT_NAME', 'claude'),
    model: envStr('CLAUDE_MODEL', 'sonnet'),
    cwd: envStr('CLAUDE_CWD', process.cwd()),
    pollIntervalMs: envInt('CLAUDE_POLL_INTERVAL', 1000),
    effort: envStr('CLAUDE_EFFORT', 'high') as 'low' | 'medium' | 'high' | 'xhigh' | 'max',
    sessionStorePath: envStr('CLAUDE_SESSION_STORE_PATH', os.homedir() + '/.wechat-gateway/claude-sessions.json'),
    httpProxy: envStrOrUndefined('HTTP_PROXY') ?? envStrOrUndefined('http_proxy'),
    httpsProxy: envStrOrUndefined('HTTPS_PROXY') ?? envStrOrUndefined('https_proxy'),
    autoCompactWindow: envIntOrUndefined('CLAUDE_AUTO_COMPACT_WINDOW'),
  };
}

function envIntOrUndefined(name: string): number | undefined {
  const v = process.env[name]?.trim();
  if (!v) return undefined;
  const n = parseInt(v, 10);
  return Number.isNaN(n) ? undefined : n;
}

function envStr(name: string, fallback: string): string {
  return process.env[name]?.trim() || fallback;
}

function envStrOrUndefined(name: string): string | undefined {
  const v = process.env[name]?.trim();
  return v || undefined;
}

function envInt(name: string, fallback: number): number {
  const v = process.env[name]?.trim();
  if (!v) return fallback;
  const n = parseInt(v, 10);
  return Number.isNaN(n) ? fallback : n;
}
