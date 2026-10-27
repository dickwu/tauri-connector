// A connected socket alone can precede the native document's load completion.
// Health checks the actual target and page without installing a runtime.
// Plugins before 0.17.3 called the cold state `runtime_missing`.
export const COLD_RUNTIME=['runtime_not_installed','runtime_missing'];
export function coldRuntime(health) {
  return COLD_RUNTIME.includes(health?.conclusion);
}
export function inspectionTargetReady(health) {
  return health?.checks?.bridge?.status==='responsive' &&
    (coldRuntime(health)||health.conclusion==='runtime_responsive');
}
