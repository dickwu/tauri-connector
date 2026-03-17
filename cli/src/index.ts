#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'fs'
import { tmpdir } from 'os'
import { join } from 'path'
import { ConnectorClient } from './client.js'
import { buildSnapshotScript, parseRef, type RefEntry } from './snapshot.js'

const DEFAULT_HOST = process.env.TAURI_CONNECTOR_HOST ?? '127.0.0.1'
const DEFAULT_PORT = parseInt(process.env.TAURI_CONNECTOR_PORT ?? '9555', 10)
const REF_CACHE_PATH = join(tmpdir(), 'tauri-connector-refs.json')

// Persistent ref map — loaded from disk, saved after snapshot
let refMap: Record<string, RefEntry> = {}

function loadRefs(): void {
  try {
    const data = readFileSync(REF_CACHE_PATH, 'utf-8')
    refMap = JSON.parse(data)
  } catch {
    refMap = {}
  }
}

function saveRefs(): void {
  writeFileSync(REF_CACHE_PATH, JSON.stringify(refMap))
}

loadRefs()

const client = new ConnectorClient()

// ============ Helpers ============

async function ensureConnected(): Promise<void> {
  try {
    await client.send({ type: 'ping' }, 3000)
  } catch {
    await client.connect(DEFAULT_HOST, DEFAULT_PORT)
  }
}

async function execJs(script: string, timeoutMs = 30_000): Promise<unknown> {
  return client.send({ type: 'execute_js', script, window_id: 'main' }, timeoutMs)
}

function resolveTarget(selectorOrRef: string): string {
  const ref = parseRef(selectorOrRef)
  if (!ref) return selectorOrRef // treat as CSS selector

  const entry = refMap[ref]
  if (!entry) {
    throw new Error(`Unknown ref: ${ref}. Run 'snapshot' first.`)
  }

  // Build JS to find the element by multiple strategies
  return `__ref_${ref}`
}

function buildResolveAndActScript(selectorOrRef: string, actionJs: string): string {
  const ref = parseRef(selectorOrRef)
  if (!ref) {
    // CSS selector path
    return `(() => {
      const el = document.querySelector("${selectorOrRef.replace(/"/g, '\\"')}");
      if (!el) return { error: 'Element not found: ${selectorOrRef.replace(/'/g, "\\'")}' };
      ${actionJs}
    })()`
  }

  const entry = refMap[ref]
  if (!entry) {
    return `(() => { return { error: 'Unknown ref: ${ref}. Run snapshot first.' }; })()`
  }

  // Multi-strategy element resolution
  const { tag, role, name, selector } = entry
  const escapedName = (name || '').replace(/"/g, '\\"').substring(0, 50)

  return `(() => {
    let el = null;

    // Strategy 1: CSS selector
    el = document.querySelector("${selector.replace(/"/g, '\\"')}");

    // Strategy 2: role + accessible name matching
    if (!el) {
      const candidates = document.querySelectorAll("${tag}");
      for (const c of candidates) {
        const r = c.getAttribute('role');
        const al = c.getAttribute('aria-label') || '';
        const t = c.textContent?.trim().substring(0, 100) || '';
        if ((al.includes("${escapedName}") || t.includes("${escapedName}"))) {
          el = c; break;
        }
      }
    }

    // Strategy 3: all elements with matching role
    if (!el && "${role}") {
      const byRole = document.querySelectorAll('[role="${role}"]');
      for (const c of byRole) {
        const al = c.getAttribute('aria-label') || '';
        const t = c.textContent?.trim().substring(0, 100) || '';
        if (al.includes("${escapedName}") || t.includes("${escapedName}")) {
          el = c; break;
        }
      }
    }

    if (!el) return { error: 'Could not resolve ref=${ref} (${tag} "${escapedName}")' };
    ${actionJs}
  })()`
}

// ============ Commands ============

async function cmdSnapshot(args: string[]): Promise<void> {
  let interactive = false
  let compact = false
  let maxDepth = 0
  let selector: string | undefined

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '-i' || args[i] === '--interactive') interactive = true
    else if (args[i] === '-c' || args[i] === '--compact') compact = true
    else if ((args[i] === '-d' || args[i] === '--depth') && args[i + 1]) maxDepth = parseInt(args[++i], 10)
    else if ((args[i] === '-s' || args[i] === '--selector') && args[i + 1]) selector = args[++i]
  }

  const script = buildSnapshotScript({ interactive, compact, maxDepth, selector })
  const result = await execJs(script) as { snapshot: string; refs: Record<string, RefEntry> } | null

  if (!result || typeof result !== 'object') {
    console.error('Snapshot failed')
    process.exit(1)
  }

  refMap = result.refs
  saveRefs()
  console.log(result.snapshot)
  console.error(`\n${Object.keys(refMap).length} refs captured`)
}

async function cmdClick(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: click <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    const rect = el.getBoundingClientRect();
    el.click();
    return { action: 'click', tag: el.tagName.toLowerCase(), x: rect.x + rect.width/2, y: rect.y + rect.height/2 };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdDblclick(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: dblclick <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    el.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    return { action: 'dblclick', tag: el.tagName.toLowerCase() };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdHover(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: hover <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    el.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
    el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    const rect = el.getBoundingClientRect();
    return { action: 'hover', tag: el.tagName.toLowerCase(), x: rect.x + rect.width/2, y: rect.y + rect.height/2 };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdFocus(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: focus <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    el.focus();
    return { action: 'focus', tag: el.tagName.toLowerCase() };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdFill(args: string[]): Promise<void> {
  const target = args[0]
  const value = args.slice(1).join(' ')
  if (!target || !value) { console.error('Usage: fill <@ref|selector> <text>'); process.exit(1) }

  const escapedValue = value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
  const script = buildResolveAndActScript(target, `
    el.focus();
    if (el.select) el.select();
    el.value = "";
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.value = "${escapedValue}";
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return { action: 'fill', tag: el.tagName.toLowerCase(), value: "${escapedValue}" };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdType(args: string[]): Promise<void> {
  const target = args[0]
  const text = args.slice(1).join(' ')
  if (!target || !text) { console.error('Usage: type <@ref|selector> <text>'); process.exit(1) }

  const escapedText = text.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
  const script = buildResolveAndActScript(target, `
    el.focus();
    const chars = "${escapedText}";
    for (const ch of chars) {
      el.dispatchEvent(new KeyboardEvent('keydown', { key: ch, bubbles: true }));
      el.dispatchEvent(new KeyboardEvent('keypress', { key: ch, bubbles: true }));
      if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
        el.value += ch;
        el.dispatchEvent(new Event('input', { bubbles: true }));
      }
      el.dispatchEvent(new KeyboardEvent('keyup', { key: ch, bubbles: true }));
    }
    return { action: 'type', tag: el.tagName.toLowerCase(), text: "${escapedText}" };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdCheck(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: check <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    if (!el.checked) el.click();
    return { action: 'check', checked: el.checked };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdUncheck(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: uncheck <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    if (el.checked) el.click();
    return { action: 'uncheck', checked: el.checked };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdSelect(args: string[]): Promise<void> {
  const target = args[0]
  const values = args.slice(1)
  if (!target || values.length === 0) { console.error('Usage: select <@ref|selector> <value...>'); process.exit(1) }

  const valuesJson = JSON.stringify(values)
  const script = buildResolveAndActScript(target, `
    const vals = ${valuesJson};
    const options = el.querySelectorAll('option');
    let matched = [];
    options.forEach(opt => {
      if (vals.includes(opt.value) || vals.includes(opt.textContent.trim())) {
        opt.selected = true;
        matched.push(opt.value);
      }
    });
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return { action: 'select', matched };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdScroll(args: string[]): Promise<void> {
  const direction = args[0] || 'down'
  const amount = parseInt(args[1] || '300', 10)
  let target = ''
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--selector' && args[i + 1]) target = args[++i]
  }

  const dx = direction === 'left' ? -amount : direction === 'right' ? amount : 0
  const dy = direction === 'up' ? -amount : direction === 'down' ? amount : 0

  if (target) {
    const script = buildResolveAndActScript(target, `
      el.scrollBy(${dx}, ${dy});
      return { action: 'scroll', direction: '${direction}', amount: ${amount} };
    `)
    const result = await execJs(script)
    console.log(JSON.stringify(result, null, 2))
  } else {
    const result = await execJs(`(() => { window.scrollBy(${dx}, ${dy}); return { action: 'scroll', direction: '${direction}', amount: ${amount} }; })()`)
    console.log(JSON.stringify(result, null, 2))
  }
}

async function cmdScrollIntoView(args: string[]): Promise<void> {
  const target = args[0]
  if (!target) { console.error('Usage: scrollintoview <@ref|selector>'); process.exit(1) }

  const script = buildResolveAndActScript(target, `
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    return { action: 'scrollintoview', tag: el.tagName.toLowerCase() };
  `)
  const result = await execJs(script)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdPress(args: string[]): Promise<void> {
  const key = args[0]
  if (!key) { console.error('Usage: press <key>'); process.exit(1) }

  const result = await execJs(`(() => {
    const el = document.activeElement || document.body;
    el.dispatchEvent(new KeyboardEvent('keydown', { key: '${key}', bubbles: true }));
    el.dispatchEvent(new KeyboardEvent('keyup', { key: '${key}', bubbles: true }));
    return { action: 'press', key: '${key}', target: el.tagName.toLowerCase() };
  })()`)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdGet(args: string[]): Promise<void> {
  const prop = args[0]
  const target = args[1]

  if (prop === 'title') {
    const result = await execJs('document.title')
    console.log(result)
    return
  }

  if (prop === 'url') {
    const result = await execJs('location.href')
    console.log(result)
    return
  }

  if (!target) { console.error('Usage: get <text|html|value|attr|box|styles> <@ref|selector> [attr-name]'); process.exit(1) }

  const actions: Record<string, string> = {
    text: 'return el.textContent.trim();',
    html: 'return el.innerHTML;',
    value: 'return el.value || el.getAttribute("aria-valuenow") || "";',
    box: 'const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height };',
    styles: `const cs = getComputedStyle(el); const s = {}; for (let i = 0; i < cs.length; i++) { s[cs[i]] = cs.getPropertyValue(cs[i]); } return s;`,
    attr: `return el.getAttribute("${(args[2] || '').replace(/"/g, '\\"')}");`,
    count: '', // handled separately
  }

  if (prop === 'count') {
    const result = await execJs(`document.querySelectorAll("${target.replace(/"/g, '\\"')}").length`)
    console.log(result)
    return
  }

  const actionJs = actions[prop]
  if (!actionJs) { console.error(`Unknown property: ${prop}`); process.exit(1) }

  const script = buildResolveAndActScript(target, actionJs)
  const result = await execJs(script)
  console.log(typeof result === 'string' ? result : JSON.stringify(result, null, 2))
}

async function cmdWait(args: string[]): Promise<void> {
  let selector: string | undefined
  let text: string | undefined
  let timeout = 5000

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--text' && args[i + 1]) text = args[++i]
    else if (args[i] === '--timeout' && args[i + 1]) timeout = parseInt(args[++i], 10)
    else if (/^\d+$/.test(args[i])) timeout = parseInt(args[i], 10)
    else selector = args[i]
  }

  const result = await client.send({
    type: 'wait_for',
    selector,
    text,
    timeout,
    window_id: 'main',
  }, timeout + 5000)
  console.log(JSON.stringify(result, null, 2))
}

async function cmdEval(args: string[]): Promise<void> {
  const script = args.join(' ')
  if (!script) { console.error('Usage: eval <js-expression>'); process.exit(1) }
  const result = await execJs(script)
  console.log(typeof result === 'string' ? result : JSON.stringify(result, null, 2))
}

async function cmdLogs(args: string[]): Promise<void> {
  let lines = 20
  let filter: string | undefined
  for (let i = 0; i < args.length; i++) {
    if ((args[i] === '-n' || args[i] === '--lines') && args[i + 1]) lines = parseInt(args[++i], 10)
    else if ((args[i] === '-f' || args[i] === '--filter') && args[i + 1]) filter = args[++i]
  }
  const result = await execJs(`(() => {
    const logs = (window.__CONNECTOR_LOGS__ || [])${filter ? `.filter(l => l.message.toLowerCase().includes("${filter.toLowerCase().replace(/"/g, '\\"')}"))` : ''};
    return logs.slice(-${lines});
  })()`)
  if (Array.isArray(result)) {
    for (const entry of result as Array<{ level: string; message: string; timestamp: number }>) {
      const time = new Date(entry.timestamp).toLocaleTimeString()
      const level = entry.level.toUpperCase().padEnd(5)
      console.log(`${time} ${level} ${entry.message}`)
    }
  } else {
    console.log(JSON.stringify(result, null, 2))
  }
}

async function cmdState(): Promise<void> {
  const result = await client.send({ type: 'backend_state' })
  console.log(JSON.stringify(result, null, 2))
}

async function cmdWindows(): Promise<void> {
  const result = await client.send({ type: 'window_list' })
  console.log(JSON.stringify(result, null, 2))
}

// ============ Help ============

function printHelp(): void {
  console.log(`
tauri-connector CLI - interact with Tauri apps

USAGE:
  tauri-connector <command> [args...]

CONNECTION:
  Connects to tauri-plugin-connector WebSocket on
  \${TAURI_CONNECTOR_HOST}:\${TAURI_CONNECTOR_PORT} (default: 127.0.0.1:9555)

SNAPSHOT:
  snapshot [-i] [-c] [-d N] [-s "#selector"]
    Take DOM snapshot with ref IDs. Flags:
      -i  Interactive only (elements with refs)
      -c  Compact (remove structural wrappers)
      -d  Max depth
      -s  Scope to CSS selector

INTERACTIONS (use @eN refs from snapshot):
  click <@ref|selector>              Click an element
  dblclick <@ref|selector>           Double-click
  hover <@ref|selector>              Hover over element
  focus <@ref|selector>              Focus an element
  fill <@ref|selector> <text>        Clear and fill input
  type <@ref|selector> <text>        Type text character by character
  check <@ref|selector>              Check checkbox
  uncheck <@ref|selector>            Uncheck checkbox
  select <@ref|selector> <val...>    Select option(s)
  scrollintoview <@ref|selector>     Scroll element into view

KEYBOARD:
  press <key>                        Press a key (Enter, Tab, Escape, etc.)

SCROLL:
  scroll [up|down|left|right] [amount] [--selector <sel>]

GETTERS:
  get title                          Page title
  get url                            Current URL
  get text <@ref|selector>           Text content
  get html <@ref|selector>           Inner HTML
  get value <@ref|selector>          Input value
  get attr <@ref|selector> <name>    Attribute value
  get box <@ref|selector>            Bounding box
  get styles <@ref|selector>         Computed styles
  get count <selector>               Element count

WAIT:
  wait <selector>                    Wait for element
  wait <ms>                          Wait for duration
  wait --text "Success"              Wait for text

OTHER:
  eval <js-expression>               Execute JavaScript
  logs [-n 20] [-f "filter"]         Console logs
  state                              App metadata
  windows                            List windows
  help                               This help

EXAMPLES:
  tauri-connector snapshot
  tauri-connector snapshot -i -c
  tauri-connector click @e7
  tauri-connector fill @e5 "user@example.com"
  tauri-connector get text @e4
  tauri-connector press Enter
  tauri-connector eval "document.title"
`)
}

// ============ Main ============

async function main(): Promise<void> {
  const args = process.argv.slice(2)
  if (args.length === 0 || args[0] === 'help' || args[0] === '--help' || args[0] === '-h') {
    printHelp()
    return
  }

  const command = args[0]
  const commandArgs = args.slice(1)

  await ensureConnected()

  const commands: Record<string, (args: string[]) => Promise<void>> = {
    snapshot: cmdSnapshot,
    click: cmdClick,
    dblclick: cmdDblclick,
    hover: cmdHover,
    focus: cmdFocus,
    fill: cmdFill,
    type: cmdType,
    check: cmdCheck,
    uncheck: cmdUncheck,
    select: cmdSelect,
    scroll: cmdScroll,
    scrollintoview: cmdScrollIntoView,
    press: cmdPress,
    key: cmdPress,
    get: cmdGet,
    wait: cmdWait,
    eval: cmdEval,
    logs: cmdLogs,
    state: cmdState,
    windows: cmdWindows,
  }

  const handler = commands[command]
  if (!handler) {
    console.error(`Unknown command: ${command}. Run 'tauri-connector help' for usage.`)
    process.exit(1)
  }

  try {
    await handler(commandArgs)
  } catch (err) {
    console.error(`Error: ${err instanceof Error ? err.message : String(err)}`)
    process.exit(1)
  }

  client.disconnect()
}

main()
