# Inspection runtime transport measurements

Measured on the isolated macOS WKWebView fixture for the 0.16.0 release candidate. Machine-readable samples: [inspection-upgrade.json](inspection-upgrade.json). No production application or business data was used.

## Experiment

The two warm modes use the same installed runtime, semantic/context checks, fill/value postconditions and independent DOM input counters. Full-source retransmits the cached workflow factory; short-packet omits that source. This isolates transport/parsing overhead, not the performance of two released binaries. Mode order alternates. All 30samples per configuration are retained; setup is excluded while required validation stays in the timed workload.

## Warm results

| Steps | Mode | Samples | Median ms | P95 ms | Mean script bytes | Mean transmitted command bytes |
| --- | --- | --- | --- | --- | --- | --- |
| 2 | full_source | 30 | 11.01 | 13.54 | 123871 | 127143 |
| 2 | short_packet | 30 | 3.75 | 5.41 | 6151 | 7578 |
| 10 | full_source | 30 | 77.44 | 235.14 | 421665 | 431577 |
| 10 | short_packet | 30 | 27.99 | 208.92 | 21417 | 25056 |
| 20 | full_source | 30 | 139.79 | 288.34 | 793916 | 812128 |
| 20 | short_packet | 30 | 40.57 | 66.18 | 40508 | 46912 |

Each warm sample added zero bundle installations, preserved its expected input-event count, and caused zero unintended Rust saves. Native fault tests separately show that unknown results do not replay. The latency tails vary substantially in this run; these data do not establish production tail latency.

## Cold service samples

Cold follows a fresh document reload and a read-only missing-runtime check. It includes the full workflow service, journal/preparation and postconditions. One sample per size is reported without a cold P95 or a speed ratio against the warm microbenchmark.

| Steps | Samples | Service ms | Installations | Bundle bytes | Command bytes | Input events |
| --- | --- | --- | --- | --- | --- | --- |
| 2 | 1 | 71.13 | 1 | 100875 | 20684 | 1 |
| 10 | 1 | 249.54 | 1 | 100875 | 114125 | 5 |
| 20 | 1 | 572.86 | 1 | 100875 | 255950 | 10 |

## Capture and screenshot costs

The separate [capture off/on experiment](capture-benchmark.md) measures the same GUI workflow load with 30samples per mode. It records independent writes, queue/drop/pending observations and bounded cleanup.

The latest OS-selected element has these individually measured native screenshot stages ([source JSON](../upgrade-evidence/macos-screenshot-timings.json)):

| Stage | Status | Elapsed ms |
| --- | --- | --- |
| capture | measured | 4.635208 |
| decode | measured | 100.905125 |
| mask | measured | 0.742333 |
| cropResize | measured | 0.355292 |
| encode | measured | 2.833166 |
| protectedMemoryStore | measured | 0.008125 |
| formatConversion | not_applicable (png_output) | not measured / not applicable |
| diskSave | not_applicable (protected_memory_only) | not measured / not applicable |

Disk persistence was not performed: protected-memory storage is the current policy. Its unavailable/not-applicable status must not be represented as a zero-millisecond save. These are one capture's stage timings, not stage distributions.

## Limits and reproduction

Measurements cover one macOS main window. Multi-window performance, other platforms, preview-policy overhead, and process RSS were not measured. Reservation accounting is not total RSS. Thirty warm samples and one cold sample per size do not establish production tail behavior. Runtime source hash and every raw sample are retained in the JSON.

```sh
node examples/workflow-fixture/scripts/prepare.mjs
cargo build --manifest-path examples/workflow-fixture/Cargo.toml --locked --target-dir target
CONNECTOR_NATIVE_BENCHMARK=1 node examples/workflow-fixture/scripts/upgrade-native-test.mjs
```
