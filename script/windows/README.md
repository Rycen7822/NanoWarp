# Windows installer

`windows-installer.iss` builds the NanoWarp installer with Inno Setup 6.7 or
newer. It installs `nanowarp.exe` under NanoWarp's own directory, registry and
shortcut identity. Cargo's development target remains `warp-oss`.

## Build

Run from the repository root after loading the local Rust environment:

```powershell
. .\.work\rust-env.ps1
$env:CARGO_BUILD_JOBS = '2'
$env:RUST_TEST_THREADS = '1'
.\script\windows\bundle.ps1 -DEBUG_BUILD -RELEASE_TAG 0.1.0
```

Put `ISCC.exe` on the current process's PATH. Keep tool downloads, TEMP/TMP,
Cargo output and installer output under the D-drive `.work` directory.
The [official Inno Setup notes](https://jrsoftware.org/ishelp/topic_technotes.htm)
describe portable installation of the compiler itself.

The default installer is unsigned. To use your own Authenticode signing setup,
pass `-SIGN_TOOL_CMD`; this is independent of Git commit signing.

Use `-SKIP_BUILD_BINARY` only with an existing package-named executable and
`SETTINGS_SCHEMA_SOURCE` or `SETTINGS_SCHEMA_EXECUTABLE`. Cross-compiled binaries
also require a runnable schema source; see the script's existing guards.
`-CHECK_ONLY` checks the Cargo target without building an installer.

The bundler supplies `TargetProfileDir`, `MyAppName`, `MyAppExeName`,
`MyAppVersion`, `ReleaseChannel`, `Arch` and `OutputName` to ISCC. It writes to
`.work/target/<target>/<profile>/installer/` (or the selected CARGO_TARGET_DIR).
The x64 output is `NanoWarpSetup.exe`; arm64 uses `NanoWarpSetup-arm64.exe`.

## Verification

Compile the installer and inspect its inputs before installing it. Compilation
does not verify registry writes, uninstall behavior or a running GUI.
The installer uses the NanoWarp mutex for ordinary running-app checks; it has
no background-update parameter, wait loop or image-name process kill.

Icons come from `app/channels/oss/icon/no-padding/icon.ico`; shared ConPTY,
OpenConsole, VC runtime and graphics DLL filenames remain unchanged.
