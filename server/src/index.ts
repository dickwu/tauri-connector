#!/usr/bin/env node

import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { z } from 'zod'
import { ConnectorClient } from './client.js'

const DEFAULT_PORT = 9555
const DEFAULT_HOST = '127.0.0.1'

function createServer() {
  const host = process.env.TAURI_CONNECTOR_HOST ?? DEFAULT_HOST
  const port = parseInt(process.env.TAURI_CONNECTOR_PORT ?? String(DEFAULT_PORT), 10)

  const client = new ConnectorClient(host, port)
  const server = new McpServer({
    name: 'tauri-connector',
    version: '0.1.0',
  })

  const textResult = (data: unknown) => ({
    content: [{ type: 'text' as const, text: typeof data === 'string' ? data : JSON.stringify(data, null, 2) }],
  })

  // ===== Session Management =====

  server.tool(
    'driver_session',
    'Start/stop connection to a running Tauri app',
    { action: z.enum(['start', 'stop', 'status']), host: z.string().optional(), port: z.number().optional() },
    async ({ action, host: h, port: p }) => {
      if (action === 'start') {
        await client.connect(h ?? host, p ?? port)
        return textResult(`Connected to ${h ?? host}:${p ?? port}`)
      }
      if (action === 'stop') {
        client.disconnect()
        return textResult('Disconnected')
      }
      return textResult(client.isConnected() ? `Connected to ${host}:${port}` : 'Not connected')
    },
  )

  // ===== JavaScript Execution =====

  server.tool(
    'webview_execute_js',
    'Execute JavaScript in the Tauri webview. Use IIFE for return values: "(() => { return value; })()"',
    { script: z.string(), windowId: z.string().optional() },
    async ({ script, windowId }) => {
      const result = await client.send({ type: 'execute_js', script, window_id: windowId ?? 'main' })
      return textResult(result)
    },
  )

  // ===== Screenshot =====

  server.tool(
    'webview_screenshot',
    'Take a screenshot of the Tauri window using native xcap capture (cross-platform)',
    {
      format: z.enum(['png', 'jpeg', 'webp']).optional(),
      quality: z.number().min(0).max(100).optional(),
      maxWidth: z.number().optional(),
      windowId: z.string().optional(),
    },
    async ({ format, quality, maxWidth, windowId }) => {
      const result = await client.send({
        type: 'screenshot',
        format: format ?? 'jpeg',
        quality: quality ?? 80,
        max_width: maxWidth,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== DOM Snapshot =====

  server.tool(
    'webview_dom_snapshot',
    'Get structured DOM snapshot (accessibility or structure tree) via JS bridge',
    {
      type: z.enum(['accessibility', 'structure']),
      selector: z.string().optional(),
      windowId: z.string().optional(),
    },
    async ({ type: snapshotType, selector, windowId }) => {
      const result = await client.send({
        type: 'dom_snapshot',
        snapshot_type: snapshotType,
        selector,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== Cached DOM (pushed from frontend via invoke) =====

  server.tool(
    'get_cached_dom',
    'Get cached DOM snapshot that was pushed from the frontend via invoke(). Faster and more LLM-friendly than webview_dom_snapshot. Includes HTML, text content, accessibility tree, and structure tree.',
    { windowId: z.string().optional() },
    async ({ windowId }) => {
      const result = await client.send({ type: 'get_cached_dom', window_id: windowId ?? 'main' })
      return textResult(result)
    },
  )

  // ===== Find Element =====

  server.tool(
    'webview_find_element',
    'Find elements by CSS selector, XPath, or text content',
    {
      selector: z.string(),
      strategy: z.enum(['css', 'xpath', 'text']).optional(),
      windowId: z.string().optional(),
    },
    async ({ selector, strategy, windowId }) => {
      const result = await client.send({
        type: 'find_element',
        selector,
        strategy: strategy ?? 'css',
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== Get Styles =====

  server.tool(
    'webview_get_styles',
    'Get computed CSS styles for an element',
    {
      selector: z.string(),
      properties: z.array(z.string()).optional(),
      windowId: z.string().optional(),
    },
    async ({ selector, properties, windowId }) => {
      const result = await client.send({
        type: 'get_styles',
        selector,
        properties,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== Interact (click, scroll, hover, focus) =====

  server.tool(
    'webview_interact',
    'Perform gestures: click, double-click, focus, scroll, hover on elements',
    {
      action: z.enum(['click', 'double-click', 'dblclick', 'focus', 'scroll', 'hover']),
      selector: z.string().optional(),
      strategy: z.enum(['css', 'xpath', 'text']).optional(),
      x: z.number().optional(),
      y: z.number().optional(),
      direction: z.enum(['up', 'down', 'left', 'right']).optional(),
      distance: z.number().optional(),
      windowId: z.string().optional(),
    },
    async ({ action, selector, strategy, x, y, direction, distance, windowId }) => {
      const result = await client.send({
        type: 'interact',
        action,
        selector,
        strategy: strategy ?? 'css',
        x, y, direction, distance,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== Keyboard =====

  server.tool(
    'webview_keyboard',
    'Type text or press keys with optional modifiers',
    {
      action: z.enum(['type', 'press']),
      text: z.string().optional(),
      key: z.string().optional(),
      modifiers: z.array(z.enum(['ctrl', 'shift', 'alt', 'meta'])).optional(),
      windowId: z.string().optional(),
    },
    async ({ action, text, key, modifiers, windowId }) => {
      const result = await client.send({
        type: 'keyboard',
        action,
        text, key, modifiers,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== Wait For =====

  server.tool(
    'webview_wait_for',
    'Wait for element selectors or text content to appear',
    {
      selector: z.string().optional(),
      strategy: z.enum(['css', 'xpath', 'text']).optional(),
      text: z.string().optional(),
      timeout: z.number().optional(),
      windowId: z.string().optional(),
    },
    async ({ selector, strategy, text, timeout, windowId }) => {
      const result = await client.send({
        type: 'wait_for',
        selector,
        strategy: strategy ?? 'css',
        text,
        timeout: timeout ?? 5000,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  // ===== Get Pointed Element (Alt+Shift+Click) =====

  server.tool(
    'webview_get_pointed_element',
    'Get metadata for the element the user Alt+Shift+Clicked',
    { windowId: z.string().optional() },
    async ({ windowId }) => {
      const result = await client.send({ type: 'get_pointed_element', window_id: windowId ?? 'main' })
      return textResult(result)
    },
  )

  // ===== Window Management =====

  server.tool(
    'manage_window',
    'List windows, get window info, or resize a window',
    {
      action: z.enum(['list', 'info', 'resize']),
      windowId: z.string().optional(),
      width: z.number().optional(),
      height: z.number().optional(),
    },
    async ({ action, windowId, width, height }) => {
      let cmd: Record<string, unknown>
      if (action === 'list') {
        cmd = { type: 'window_list' }
      } else if (action === 'info') {
        cmd = { type: 'window_info', window_id: windowId ?? 'main' }
      } else {
        if (!width || !height) return textResult('Error: width and height required for resize')
        cmd = { type: 'window_resize', window_id: windowId ?? 'main', width, height }
      }
      const result = await client.send(cmd)
      return textResult(result)
    },
  )

  // ===== Backend State =====

  server.tool(
    'ipc_get_backend_state',
    'Get Tauri app metadata, version, environment, and window info',
    {},
    async () => {
      const result = await client.send({ type: 'backend_state' })
      return textResult(result)
    },
  )

  // ===== IPC Execute Command =====

  server.tool(
    'ipc_execute_command',
    'Execute any Tauri IPC command via invoke()',
    {
      command: z.string(),
      args: z.record(z.unknown()).optional(),
    },
    async ({ command, args }) => {
      const result = await client.send({ type: 'ipc_execute_command', command, args })
      return textResult(result)
    },
  )

  // ===== IPC Monitor =====

  server.tool(
    'ipc_monitor',
    'Start or stop IPC monitoring to capture invoke() calls',
    { action: z.enum(['start', 'stop']) },
    async ({ action }) => {
      const result = await client.send({ type: 'ipc_monitor', action })
      return textResult(result)
    },
  )

  // ===== IPC Get Captured =====

  server.tool(
    'ipc_get_captured',
    'Retrieve captured IPC traffic (requires monitoring to be started)',
    {
      filter: z.string().optional(),
      limit: z.number().optional(),
    },
    async ({ filter, limit }) => {
      const result = await client.send({ type: 'ipc_get_captured', filter, limit: limit ?? 100 })
      return textResult(result)
    },
  )

  // ===== IPC Emit Event =====

  server.tool(
    'ipc_emit_event',
    'Emit a custom Tauri event for testing event handlers',
    {
      eventName: z.string(),
      payload: z.unknown().optional(),
    },
    async ({ eventName, payload }) => {
      const result = await client.send({ type: 'ipc_emit_event', event_name: eventName, payload })
      return textResult(result)
    },
  )

  // ===== Console Logs =====

  server.tool(
    'read_logs',
    'Read captured console logs from the webview',
    {
      lines: z.number().optional(),
      filter: z.string().optional(),
      windowId: z.string().optional(),
    },
    async ({ lines, filter, windowId }) => {
      const result = await client.send({
        type: 'console_logs',
        lines: lines ?? 50,
        filter,
        window_id: windowId ?? 'main',
      })
      return textResult(result)
    },
  )

  return server
}

async function main() {
  const server = createServer()
  const transport = new StdioServerTransport()
  await server.connect(transport)
  console.error('[tauri-connector-mcp] Server started on stdio')
}

main().catch((err) => {
  console.error('Fatal:', err)
  process.exit(1)
})
