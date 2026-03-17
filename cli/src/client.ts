import WebSocket from 'ws'
import { randomUUID } from 'crypto'

interface PendingRequest {
  resolve: (value: unknown) => void
  reject: (reason: Error) => void
  timer: ReturnType<typeof setTimeout>
}

export class ConnectorClient {
  private ws: WebSocket | null = null
  private pending = new Map<string, PendingRequest>()

  async connect(host: string, port: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const url = `ws://${host}:${port}`
      this.ws = new WebSocket(url)
      this.ws.on('open', () => resolve())
      this.ws.on('error', (err) => reject(new Error(`Connection failed: ${err.message}`)))
      this.ws.on('message', (data) => this.handleMessage(data.toString()))
      this.ws.on('close', () => {
        this.rejectAll('Connection closed')
        this.ws = null
      })
    })
  }

  disconnect(): void {
    if (this.ws) {
      this.rejectAll('Disconnected')
      this.ws.close()
      this.ws = null
    }
  }

  async send(command: Record<string, unknown>, timeoutMs = 35_000): Promise<unknown> {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      throw new Error('Not connected')
    }
    const id = randomUUID()
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error('Request timeout'))
      }, timeoutMs)
      this.pending.set(id, { resolve, reject, timer })
      this.ws!.send(JSON.stringify({ id, ...command }))
    })
  }

  private handleMessage(data: string): void {
    let response: { id: string; result?: unknown; error?: string }
    try {
      response = JSON.parse(data)
    } catch {
      return
    }
    const pending = this.pending.get(response.id)
    if (!pending) return
    this.pending.delete(response.id)
    clearTimeout(pending.timer)
    if (response.error) {
      pending.reject(new Error(response.error))
    } else {
      pending.resolve(response.result)
    }
  }

  private rejectAll(reason: string): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer)
      pending.reject(new Error(reason))
      this.pending.delete(id)
    }
  }
}
