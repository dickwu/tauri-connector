# Capture overhead under the same GUI workflow load

This is a native macOS WKWebView experiment for the 0.16.0 release candidate, using the normal workflow click/fill/click/query path and an independent Rust fixture state file. The workflow input is synthetic DOM input inside the real WebView; it is not an OS-input benchmark. Raw samples: [capture-benchmark.json](capture-benchmark.json).

## Method

Both configurations use the same plugin build, four workflow steps, semantic checks, journal and GUI postconditions. Capture-off includes the installed dormant interception code. Capture-on uses metadata-only arguments/results for `fixture_create_task`, with `followPages:false`. Each mode has one warmup followed by 30 measured samples. Backend/DOM state is reset and reloaded before each block.

The order is capture-off then capture-on. Reloads and warmups equalize fixture state, but do not eliminate temporal, thermal or journal-history bias. The observed difference is therefore not a causal measurement of isolated per-invoke CPU overhead.

## Results

| Mode | Samples | Workflow median ms | Workflow P95 ms | Validated median ms | Rust saves / tasks | Frontend input events | Captured drops |
| --- | --- | --- | --- | --- | --- | --- | --- |
| capture_off | 30 | 118.05 | 162.13 | 119.47 | 30 / 30 | 30 | 0 |
| capture_on | 30 | 136.79 | 205.03 | 137.99 | 30 / 30 | 30 | 0 |

Observed workflow-median difference: +18.75 ms (15.88%). Observed P95 difference: +42.90 ms. These are descriptive results of this two-block experiment, not a production or isolated-overhead guarantee.

Each measured sample created exactly one task with one Rust save and one frontend input event. Including its warmup, each mode recorded 31 saves before reset. The benchmark completed and reset only its isolated fixture data.

## Queue, pending and cleanup

Capture-on ended with zero observed pending capture invocations and zero reported dropped events. The maximum sampled queue was 0 events / 0 bytes. These are observations before/after workflows and during drainage; they are not continuous queue high-water measurements. Capture-off pending capture data is `not_enabled`/unobserved, not an observed zero. The independent native pending counter was zero in both blocks.

Capture-on was stopped through the service and fixture reset/reload was recorded. This benchmark does not prove the separate 100-cycle picker cleanup test.

## Limits and reproduction

One window, one machine and metadata-only capture are covered. Preview policy, multi-window behavior, process RSS and native Windows/Linux GUI overhead were not measured. Thirty retained samples/configuration support this comparison only; no outlier removal was performed and no production P95 is promised. Normal durable workflow permissions remain required; the harness does not bypass the unsupported Windows private-journal boundary.

```sh
node examples/workflow-fixture/scripts/prepare.mjs
cargo build --manifest-path examples/workflow-fixture/Cargo.toml --locked --target-dir target
node examples/workflow-fixture/scripts/upgrade-native-test.mjs
```

Screenshot capture/decode/mask/crop/encode/protected-memory timings are separately recorded in [the transport report](inspection-upgrade.md#capture-and-screenshot-costs). Disk save is not applicable under the current protected-memory policy, never a synthetic timing of 0.
