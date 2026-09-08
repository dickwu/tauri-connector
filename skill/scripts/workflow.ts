#!/usr/bin/env bun
/** Forward workflow lifecycle calls to the application; never execute steps locally.
 * Usage: bun workflow.ts <run|get|cancel|resume|capabilities> '<args JSON>'
 * Use @path.json to read arguments from a file. Credentials come from the environment.
 */
import { readFileSync } from 'node:fs';
import { send } from './connector.ts';

const operations = new Set(['run', 'get', 'cancel', 'resume', 'capabilities']);

export function prepareRequest(operation: string, input: unknown, token?: string) {
  if (!operations.has(operation)) throw new Error('Expected run, get, cancel, resume or capabilities');
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('Arguments must be a JSON object');
  const args: Record<string, unknown> = { ...input };
  if (operation !== 'capabilities') {
    if (!token || Buffer.byteLength(token) < 32) throw new Error('Set TAURI_CONNECTOR_WORKFLOW_TOKEN to the host credential (at least 32 bytes)');
    args.authToken = token;
  }
  return { type: 'workflow', operation: `workflow_${operation}`, args };
}

export function reportExitCode(report: Record<string, unknown>): number {
  if (['failed', 'cancelled'].includes(String(report.status)) || report.originalTestVerdict === 'failed' || report.goalStatus === 'failed') return 1;
  if (report.status !== 'completed' || report.originalTestVerdict === 'inconclusive' || report.goalStatus === 'inconclusive') return 2;
  return 0;
}

if (import.meta.main) {
  try {
    const [operation = '', source = '{}'] = process.argv.slice(2);
    const args = JSON.parse(source.startsWith('@') ? readFileSync(source.slice(1), 'utf8') : source);
    const request = prepareRequest(operation, args, process.env.TAURI_CONNECTOR_WORKFLOW_TOKEN);
    const capabilities = await send({ type: 'bridge_status' }, 5000);
    if ((capabilities.result as Record<string, unknown> | undefined)?.workflowProtocolVersion !== 1) {
      throw new Error('capability_unavailable: the application needs tauri-plugin-connector 0.15 or newer');
    }
    const response = await send(request, 35000);
    if (response.error !== undefined) {
      console.error(JSON.stringify({ error: response.error, outcome: response.outcome }));
      process.exitCode = 1;
    } else {
      const report = response.result as Record<string, unknown>;
      console.log(JSON.stringify(report, null, 2));
      process.exitCode = operation === 'capabilities' ? 0 : reportExitCode(report);
    }
  } catch (error) {
    console.error(error instanceof SyntaxError ? 'Invalid workflow arguments JSON' : String(error));
    process.exitCode = 1;
  }
}
