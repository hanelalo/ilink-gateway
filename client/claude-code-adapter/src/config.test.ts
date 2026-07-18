import os from 'node:os';
import { describe, it, expect, beforeEach } from 'vitest';
import { loadConfig } from './config.js';

beforeEach(() => {
  // Clear all relevant env vars before each test
  const envs = [
    'CLAUDE_GATEWAY_URL', 'CLAUDE_GATEWAY_AGENT_NAME', 'CLAUDE_MODEL',
    'CLAUDE_CWD', 'CLAUDE_POLL_INTERVAL', 'CLAUDE_EFFORT',
    'CLAUDE_SESSION_STORE_PATH', 'HTTP_PROXY', 'HTTPS_PROXY',
    'http_proxy', 'https_proxy',
  ];
  for (const e of envs) {
    delete process.env[e];
  }
});

describe('loadConfig', () => {
  it('should return default values when no env vars are set', () => {
    const cfg = loadConfig();
    expect(cfg.gatewayUrl).toBe('http://127.0.0.1:8765');
    expect(cfg.agentName).toBe('claude');
    expect(cfg.model).toBe('sonnet');
    expect(cfg.cwd).toBe(process.cwd());
    expect(cfg.pollIntervalMs).toBe(1000);
    expect(cfg.effort).toBe('medium');
    expect(cfg.sessionStorePath).toBe(os.homedir() + '/.wechat-gateway/claude-sessions.json');
  });

  it('should read env var values when set', () => {
    process.env.CLAUDE_GATEWAY_URL = 'http://localhost:9999';
    process.env.CLAUDE_GATEWAY_AGENT_NAME = 'my-agent';
    process.env.CLAUDE_MODEL = 'opus';
    process.env.CLAUDE_CWD = '/tmp/test';
    process.env.CLAUDE_POLL_INTERVAL = '2000';
    process.env.CLAUDE_EFFORT = 'high';
    process.env.CLAUDE_SESSION_STORE_PATH = '/tmp/sessions.json';

    const cfg = loadConfig();
    expect(cfg.gatewayUrl).toBe('http://localhost:9999');
    expect(cfg.agentName).toBe('my-agent');
    expect(cfg.model).toBe('opus');
    expect(cfg.cwd).toBe('/tmp/test');
    expect(cfg.pollIntervalMs).toBe(2000);
    expect(cfg.effort).toBe('high');
    expect(cfg.sessionStorePath).toBe('/tmp/sessions.json');
  });

  it('should parse HTTP_PROXY and HTTPS_PROXY', () => {
    process.env.HTTP_PROXY = 'http://proxy:8080';
    process.env.HTTPS_PROXY = 'https://proxy:8443';

    const cfg = loadConfig();
    expect(cfg.httpProxy).toBe('http://proxy:8080');
    expect(cfg.httpsProxy).toBe('https://proxy:8443');
  });

  it('should fall back to lowercase proxy vars', () => {
    process.env.http_proxy = 'http://lower-proxy:3128';

    const cfg = loadConfig();
    expect(cfg.httpProxy).toBe('http://lower-proxy:3128');
  });

  it('should set pollIntervalMs default for invalid input', () => {
    process.env.CLAUDE_POLL_INTERVAL = 'not-a-number';
    const cfg = loadConfig();
    expect(cfg.pollIntervalMs).toBe(1000);
  });
});
