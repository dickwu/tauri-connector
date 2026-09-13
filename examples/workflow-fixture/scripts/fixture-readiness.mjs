// A connected socket alone can precede the native document's load completion.
// Health checks the actual target and page without installing a runtime.
export function inspectionTargetReady(health) {
  return health?.checks?.bridge?.status==='responsive' &&
    ['runtime_missing','runtime_responsive'].includes(health.conclusion);
}
