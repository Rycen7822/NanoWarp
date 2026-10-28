# NanoWarp

NanoWarp is a terminal-focused fork of [Warp](https://github.com/warpdotdev/warp), built around native desktop terminal sessions and local data.

## Product Scope

- Keep terminal blocks, history, completion, tabs, splits, ordinary SSH/WSL, and basic text editing.
- Keep advanced SSH daemon support while removing its Warp account and cloud-service coupling.
- Open Markdown and Notebook files in external applications: VS Code or Zed, plus Typora for Markdown. When no application is selected, use the system file association.
- Remove the built-in AI agent, commercial workflows, cloud collaboration, full IDE/LSP, internal Notebook/Markdown preview, and Web/WASM product.

This fork is under active development. Cross-platform runtime and packaging acceptance is still in progress; source cleanup does not establish that every platform or SSH scenario has passed.

## Build from Source

Start with [Development Setup](CONTRIBUTING.md#development-setup) for prerequisites, local cache/data directories, and resource limits. From the repository root, the Windows GUI build command is:

```powershell
cargo +1.92.0 build --locked -j 2 -p warp --bin warp-oss --features release_bundle,gui
```

The Windows executable is `.work/target/debug/warp-oss.exe`. For macOS and Linux, see the platform runner described in the contribution guide. An upstream Warp download does not contain NanoWarp changes.

## Contributing and Support

Search [NanoWarp issues](https://github.com/Rycen7822/NanoWarp/issues) before reporting a bug or proposing a feature. Include reproduction steps, version, OS, expected behavior, and relevant diagnostics with secrets removed.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a [pull request](https://github.com/Rycen7822/NanoWarp/pulls). Security reporting is described in [SECURITY.md](SECURITY.md); community participation follows [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Licensing and Attribution

NanoWarp derives from Warp. Preserve applicable upstream copyrights and third-party notices.

The `warpui_core` and `warpui` UI framework crates are licensed under the [MIT license](LICENSE-MIT). The rest of this repository is licensed under the [AGPL v3](LICENSE-AGPL).
