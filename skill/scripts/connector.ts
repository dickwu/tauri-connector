/**
 * Shared WebSocket helper for tauri-connector scripts.
 *
 * Port discovery order:
 * 1. TAURI_CONNECTOR_PORT env var (explicit override)
 * 2. .connector.json PID file in target/ (auto-discovery)
 * 3. Default port 9555
 *
 * Usage: import { send, getConnectionInfo } from './connector';
 */

import { existsSync, readFileSync } from 'fs';
import { join } from 'path';

interface ConnectorInfo {
  pid: number;
  ws_port: number;
  mcp_port: number | null;
  bridge_port: number;
  app_name: string;
  app_id: string;
  exe: string;
  started_at: number;
}

const TIMEOUT = parseInt(process.env.TAURI_CONNECTOR_TIMEOUT || '15000', 10);

/** Search upward from cwd for target/.connector.json */
function findPidFile(): ConnectorInfo | null {
  // Common locations: ./target, ./src-tauri/target, ../src-tauri/target
  const searchPaths = [
    join(process.cwd(), 'target', '.connector.json'),
    join(process.cwd(), 'src-tauri', 'target', '.connector.json'),
    join(process.cwd(), '..', 'src-tauri', 'target', '.connector.json'),
    join(process.cwd(), '..', 'target', '.connector.json'),
  ];

  for (const p of searchPaths) {
    if (existsSync(p)) {
      try {
        const data = JSON.parse(readFileSync(p, 'utf-8')) as ConnectorInfo;
        // Verify process is still alive
        try {
          process.kill(data.pid, 0); // signal 0 = just check existence
          return data;
        } catch {
          // Process is dead, stale PID file
          return null;
        }
      } catch {
        return null;
      }
    }
  }
  return null;
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
