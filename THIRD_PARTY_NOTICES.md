# Third-party notices for the inspection upgrade

## Pinned screenshot reference

The screenshot implementation was informed by the platform API usage in `hypothesi/mcp-server-tauri`, commit `4451b2b1d1eb3a817674f60957e148224c24d3a8`, specifically `packages/tauri-plugin-mcp-bridge/src/screenshot/{mod,macos,windows,linux}.rs`. The LICENSE at that exact revision was checked before implementation:

https://github.com/hypothesi/mcp-server-tauri/blob/4451b2b1d1eb3a817674f60957e148224c24d3a8/LICENSE

This upgrade implements its own asynchronous capture pipeline, bounded callback ownership, raw bitmap conversion, context validation, pixel redaction, and protected artifact storage. It does not copy the reference's blocking receive or automatic window-preparation behavior. The reference notice is retained here for the platform-adapter implementation informed by that source.

```text
MIT License

Copyright (c) 2025 Fireside Development, LLC

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Screenshot binding and codec declarations

The following metadata was checked against the exact packages in `Cargo.lock` and their packaged `Cargo.toml` files on 2026-09-12. Platform bindings were already transitive Tauri/Wry dependencies; this change exposes explicit optional feature dependencies on the matching versions. These dependency declarations do not vendor their source into this repository. Their distributed packages retain their original license files.

| Package | Locked version used by screenshot integration | SPDX license |
|---|---|---|
| block2 | 0.6.2 | MIT |
| objc2 | 0.6.4 | MIT |
| objc2-app-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-core-graphics | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-foundation | 0.3.2 | MIT |
| objc2-web-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| webview2-com | 0.38.2 | MIT |
| windows | 0.61.3 | MIT OR Apache-2.0 |
| webkit2gtk | 2.0.2 | MIT |
| gio | 0.18.4 | MIT |
| cairo-rs | 0.18.5 | MIT |
| image | 0.25.10 | MIT OR Apache-2.0 |
| xcap (existing backend) | 0.9.8 | Apache-2.0 |

The wrappers call the existing platform WebView frameworks supplied through Tauri. This table describes the Rust package licenses, not a replacement license for WebKitGTK, Cairo, GTK, WebView2, or the Apple frameworks themselves.
