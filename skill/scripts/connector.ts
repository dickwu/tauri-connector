/**
 * Shared WebSocket helper for tauri-connector scripts.
 *
 * Port discovery order:
 * 1. TAURI_CONNECTOR_PORT env var (explicit override)
 * 2. TAURI_CONNECTOR_PID_FILE
 * 3. .connector.json PID files near cwd
 * 4. Default port 9555
 *
 * Usage: import { send, getConnectionInfo } from './connector';
 */

import { existsSync, readFileSync } from 'fs';
import { dirname, join, resolve } from 'path';

interface ConnectorInfo {
  pid: number;
  ws_port: number;
  mcp_port: number | null;
  bridge_port: number;
  app_name: string;
  app_id: string;
  exe: string;
  log_dir?: string;
  started_at: number;
  pid_file?: string;
}

const TIMEOUT = parseInt(process.env.TAURI_CONNECTOR_TIMEOUT || '15000', 10);

function pidFileCandidates(): string[] {
  const roots: string[] = [];
  let cur = resolve(process.cwd());
  for (let i = 0; i < 8; i += 1) {
    roots.push(cur);
    const next = dirname(cur);
    if (next === cur) break;
    cur = next;
  }

  const candidates: string[] = [];
  if (process.env.TAURI_CONNECTOR_PID_FILE) {
    candidates.push(process.env.TAURI_CONNECTOR_PID_FILE);
  }
  for (const root of roots) {
    candidates.push(
      join(root, 'src-tauri', 'target', '.connector.json'),
      join(root, 'src-tauri', 'target', 'debug', '.connector.json'),
      join(root, 'src-tauri', 'target', 'release', '.connector.json'),
      join(root, 'target', '.connector.json'),
      join(root, 'target', 'debug', '.connector.json'),
      join(root, 'target', 'release', '.connector.json'),
    );
  }
  return Array.from(new Set(candidates));
}

/** Search upward from cwd for live .connector.json files. */
function findPidFile(): ConnectorInfo | null {
  const appId = process.env.TAURI_CONNECTOR_APP_ID;
  const live: ConnectorInfo[] = [];

  for (const p of pidFileCandidates()) {
    if (existsSync(p)) {
      try {
        const data = JSON.parse(readFileSync(p, 'utf-8')) as ConnectorInfo;
        data.pid_file = p;
        if (appId && data.app_id !== appId) continue;
        // Verify process is still alive
        try {
          process.kill(data.pid, 0); // signal 0 = just check existence
          live.push(data);
        } catch {
          // Ignore stale PID files.
        }
      } catch {
        // Ignore malformed PID files and keep scanning.
      }
    }
  }
  live.sort((a, b) => (b.started_at || 0) - (a.started_at || 0));
  return live[0] || null;
}

/** Resolve host and port, preferring env vars > PID file > defaults. */
function resolveConnection(): { host: string; port: string } {
  const host = process.env.TAURI_CONNECTOR_HOST || '127.0.0.1';

  // Explicit env var takes priority
  if (process.env.TAURI_CONNECTOR_PORT) {
    return { host, port: process.env.TAURI_CONNECTOR_PORT };
  }

  // Try PID file auto-discovery
  const info = findPidFile();
  if (info) {
    return { host, port: String(info.ws_port) };
  }

  // Default
  return { host, port: '9555' };
}

const conn = resolveConnection();

/** Get the discovered connection info (or null if no PID file found). */
export function getConnectionInfo(): ConnectorInfo | null {
  return findPidFile();
}

let idCounter = 0;

export function nextId(): string {
  idCounter += 1;
  return String(idCounter);
}

export function send(
  msg: Record<string, unknown>,
  timeout = TIMEOUT,
): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const url = `ws://${conn.host}:${conn.port}`;
    const ws = new WebSocket(url);
    const timer = setTimeout(() => {
      ws.close();
      reject(new Error(`Timeout after ${timeout}ms connecting to ${url}`));
    }, timeout);

    ws.onopen = () => {
      ws.send(JSON.stringify({ id: nextId(), ...msg }));
    };
    ws.onmessage = (e: MessageEvent) => {
      clearTimeout(timer);
      const data = JSON.parse(String(e.data));
      resolve(data);
      ws.close();
    };
    ws.onerror = () => {
      clearTimeout(timer);
      reject(new Error(
        `Cannot connect to ${url}. Is the Tauri app running? ` +
        `Start it with: bun run tauri dev`
      ));
    };
  });
}

export async function sendAndPrint(
  msg: Record<string, unknown>,
  timeout = TIMEOUT,
): Promise<void> {
  const resp = await send(msg, timeout);
  if ((resp as { error?: string }).error) {
    console.error('Error:', (resp as { error: string }).error);
    process.exit(1);
  }
  console.log(JSON.stringify((resp as { result?: unknown }).result, null, 2));
}
