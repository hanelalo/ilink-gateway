import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

/**
 * Session data for a single cwd (workspace) within a user's sessions.
 */
export interface SessionEntry {
  sessionId: string | null;
  lastActive: number;
  approvedTools: string[];
}

/**
 * Per-user session data matching design doc section 3.3.
 */
export interface UserSessionData {
  aliases: Record<string, string>;
  activeCwd: string;
  sessions: Record<string, SessionEntry>;
}

/**
 * Expand a leading `~` to the user's home directory.
 */
export function expandTilde(filePath: string): string {
  if (filePath.startsWith('~')) {
    const home = os.homedir();
    if (filePath === '~') {
      return home;
    }
    return path.join(home, filePath.slice(1));
  }
  return filePath;
}

/**
 * Load all session data from the JSON file at `filePath`.
 * Returns an empty object if the file does not exist.
 * Returns an empty object and warns on corrupt JSON (SyntaxError).
 * Re-throws other errors (e.g. EACCES, EISDIR) so the caller can decide.
 */
export async function loadAll(filePath: string): Promise<Record<string, UserSessionData>> {
  const resolved = expandTilde(filePath);
  try {
    const raw = await fs.promises.readFile(resolved, 'utf-8');
    return JSON.parse(raw) as Record<string, UserSessionData>;
  } catch (err: unknown) {
    if (err instanceof SyntaxError) {
      console.warn(`Session store: corrupt JSON file at ${resolved}, starting fresh`);
      return {};
    }
    if (err instanceof Error && 'code' in err && (err as Record<string, unknown>).code === 'ENOENT') {
      return {};
    }
    throw err;
  }
}

/**
 * Save all session data to the JSON file at `filePath`.
 * Creates parent directories if they do not exist.
 */
export async function saveAll(
  data: Record<string, UserSessionData>,
  filePath: string,
): Promise<void> {
  const resolved = expandTilde(filePath);
  const dir = path.dirname(resolved);
  await fs.promises.mkdir(dir, { recursive: true });
  const json = JSON.stringify(data, null, 2);
  await fs.promises.writeFile(resolved, json, 'utf-8');
}
