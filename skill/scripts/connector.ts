/**
 * Shared WebSocket helper for tauri-connector scripts.
 * Usage: import { send } from './connector';
 */

const HOST = process.env.TAURI_CONNECTOR_HOST || '127.0.0.1';
const PORT = process.env.TAURI_CONNECTOR_PORT || '9555';
const TIMEOUT = parseInt(process.env.TAURI_CONNECTOR_TIMEOUT || '15000', 10);

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
    const ws = new WebSocket(`ws://${HOST}:${PORT}`);
    const timer = setTimeout(() => {
      ws.close();
      reject(new Error(`Timeout after ${timeout}ms`));
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
    ws.onerror = (e: Event) => {
      clearTimeout(timer);
      reject(new Error(`WebSocket error: ${(e as ErrorEvent).message || 'connection failed'}`));
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
