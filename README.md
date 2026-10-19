# GPUiForge

An independent command-line tool for generating, building and running GPUI
applications. Projects use `gpuiforge.json`; Android templates and build support
are bundled in the executable.

Android builds run on Windows, Linux and macOS hosts with a native JDK,
Android SDK/NDK and the required Rust Android targets installed.

Install from the repository root:

```sh
cargo install --path .
```

Android projects can be generated automatically or exported for manual
maintenance. Desktop and Web commands are configured as process steps.
`gpuiforge logs` follows the configured Android application's logs, with device
selection and optional priority, tag and history filters.
`gpuiforge run --logs` follows logs after launch; `gpuiforge restart --logs`
restarts an installed Android application and follows its logs.
`gpuiforge info android` displays resolved configuration and output paths.
`gpuiforge launch` starts the installed Android application without rebuilding;
`gpuiforge screenshot` saves the selected device's screen as a local PNG.
See [configuration and workflows](docs/usage.md) for setup and project ownership.

## License

MIT License. Copyright (c) 2026 cradiy. See [LICENSE](LICENSE).
