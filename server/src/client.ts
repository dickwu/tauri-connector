import WebSocket from 'ws'
import { randomUUID } from 'crypto'

interface PendingRequest {
  resolve: (value: unknown) => void
  reject: (reason: Error) => void
  timer: ReturnType<typeof setTimeout>
}

const REQUEST_TIMEOUT_MS = 35_000

export class ConnectorClient {
  private ws: WebSocket | null = null
  private pending = new Map<string, PendingRequest>()
  private host: string
  private port: number

  constructor(host: string, port: number) {
    this.host = host
    this.port = port
  }

  async connect(host?: string, port?: number): Promise<void> {
    if (host) this.host = host
    if (port) this.port = port

    this.disconnect()

    return new Promise((resolve, reject) => {
      const url = `ws://${this.host}:${this.port}`
      this.ws = new WebSocket(url)

      this.ws.on('open', () => {
        resolve()
      })

      this.ws.on('error', (err) => {
        reject(new Error(`WebSocket connection failed: ${err.message}`))
      })

      this.ws.on('message', (data) => {
        this.handleMessage(data.toString())
      })

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

  isConnected(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN
  }

  async send(command: Record<string, unknown>): Promise<unknown> {
    if (!this.isConnected()) {
      // Auto-connect on first send
      await this.connect()
    }

    const id = randomUUID()

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error('Request timeout'))
      }, REQUEST_TIMEOUT_MS)

      this.pending.set(id, { resolve, reject, timer })

      const message = JSON.stringify({ id, ...command })
      this.ws!.send(message)
    })
  }

  private handleMessage(data: string): void {
    let response: { id: string; result?: unknown; error?: string }

    try {
      response = JSON.parse(data)
    } catch {
      console.error('[client] Invalid JSON response')
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
