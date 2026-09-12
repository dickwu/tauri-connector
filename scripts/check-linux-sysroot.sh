#!/usr/bin/env bash
set -euo pipefail

# Type-check against real Debian headers from prepare-linux-check.py. This is
# not ELF linking, a Linux runtime, or a substitute for native WebKitGTK tests.
task_sysroot=$(cd "${1:?Pass the prepared sysroot root directory}" && pwd)
task_target_dir=${2:-target/linux-sysroot-check}
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_SYSROOT_DIR="$task_sysroot"
export PKG_CONFIG_LIBDIR="$task_sysroot/usr/lib/x86_64-linux-gnu/pkgconfig:$task_sysroot/usr/share/pkgconfig"
export PKG_CONFIG_PATH=
export CC_x86_64_unknown_linux_gnu="clang --target=x86_64-unknown-linux-gnu --sysroot=$task_sysroot"
export BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu="--target=x86_64-unknown-linux-gnu --sysroot=$task_sysroot"

cargo check -p tauri-plugin-connector --target x86_64-unknown-linux-gnu \
  --no-default-features --features xcap,native-screenshot --tests --locked \
  --target-dir "$task_target_dir"
cargo check --manifest-path examples/workflow-fixture/Cargo.toml \
  --target x86_64-unknown-linux-gnu --locked --target-dir "$task_target_dir"
