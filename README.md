# Chessova Desktop

Native helper for the Chessova in-browser chess review feature. Runs a headless WebSocket
server on `ws://127.0.0.1:9876/uci` that wraps Stockfish (and optionally Lc0) UCI subprocesses.

When the helper is running, [chessova.com/review](https://chessova.com/review) auto-detects it
and uses native SF + Lc0 instead of the server-side Stockfish WASM path. Higher depth, lower
latency, no rate limits.

## Distributions

| Platform | Artifact | Status |
| --- | --- | --- |
| macOS Apple Silicon | `Chessova-Desktop-<VER>-arm64.dmg` | Built via `scripts/build-dmg.sh` |
| Windows x64 | `Chessova-Desktop-<VER>-windows-x64.zip` | Built via `scripts/build-windows.sh` |
| macOS Intel | not shipped | Build from source via `cargo run --release` |
| Linux | not shipped | Build from source via `cargo run --release` |

Apple Silicon covers ~all new Macs since 2020; Intel-Mac volume is too low to justify a
separate binary. Intel-Mac and Linux users can still get full functionality by building
from source — the WebSocket protocol is the same regardless of how the helper is launched.

## Building from source

```sh
cd chessova-desktop
cargo run --release
```

Override engine paths via env:

```sh
STOCKFISH_PATH=/opt/homebrew/bin/stockfish cargo run --release
LC0_PATH=/usr/local/bin/lc0 LC0_WEIGHTS=/path/to/net cargo run --release
```

Prereqs: Rust toolchain (`rustup`) and a Stockfish binary on PATH or via env var.
Lc0 is optional — set `LC0_PATH` + `LC0_WEIGHTS` if you want the dual-engine view.

## Releasing for macOS (Apple Silicon)

One-time setup:

```sh
rustup target add aarch64-apple-darwin
brew install stockfish
```

Build:

```sh
./scripts/build-dmg.sh
```

Output: `dist/Chessova-Desktop-<version>-arm64.dmg`.

What the script does: compiles for `aarch64-apple-darwin`, bundles the host's
`/opt/homebrew/bin/stockfish` inside the `.app` (resolved at runtime via `current_exe()`),
writes `Info.plist` with `LSBackgroundOnly` so the helper runs without a Dock icon, ad-hoc
signs the bundle, and creates the DMG via `hdiutil`.

Ad-hoc-signed DMGs work but trigger a Gatekeeper warning on first open — users have to
right-click → Open. For a frictionless install, set `APPLE_DEV_ID="Developer ID Application: …"`
to sign with your $99/yr Apple Developer cert, then notarize with `xcrun notarytool`.

## Releasing for Windows x64

One-time setup (from macOS):

```sh
rustup target add x86_64-pc-windows-gnu
brew install mingw-w64
```

Build:

```sh
./scripts/build-windows.sh
```

Output: `dist/Chessova-Desktop-<version>-windows-x64.zip`.

The zip contains:

- `chessova-desktop.exe` — the helper (PE32+ console, ~6 MB)
- `stockfish.exe` — Stockfish 18, `sse41-popcnt` variant (works on every Intel/AMD CPU since
  ~2009; AVX2 would be faster but breaks pre-2013 hardware)
- `start.bat` — launcher that opens the helper with a console window
- `README.txt` — install + run instructions for the end user

Cross-compiled via MinGW from macOS — no Windows VM needed. The Stockfish binary is fetched
on first build from `github.com/official-stockfish/Stockfish/releases/tag/sf_18` and cached
in `.cache/`.

Unsigned. Windows SmartScreen will warn on first launch ("Windows protected your PC"); users
click "More info" → "Run anyway". For a clean install you'd need an EV code-signing cert
(~$300/yr from Sectigo/DigiCert).

## Wire protocol

The JSON message contract is defined in `../lib/review/uciProtocol.ts` (TypeScript source of
truth). `src/protocol.rs` mirrors every variant.

Messages are JSON text frames over WebSocket. Each frame is one complete message object.

**Client → Helper** (`ClientMessage`):

| `type` | Purpose |
|---|---|
| `hello` | Handshake; `clientVersion` string |
| `analyze` | Single-position analysis; `id`, `fen`, `engines`, `multiPv`, `depth`, `stream` |
| `analyze-batch` | Multi-position batch; `id`, `fens`, `engines`, `multiPv`, `depth` |
| `stop` | Cancel request by `id` (best-effort) |

**Helper → Client** (`HelperMessage`):

| `type` | Purpose |
|---|---|
| `hello-ack` | Lists available engines + helper version |
| `analyze-progress` | Intermediate depth result (only when `stream: true`) |
| `analyze-done` | Final result for one engine |
| `analyze-batch-progress` | How many FENs completed so far |
| `analyze-batch-done` | All batch results |
| `error` | Engine crash, missing binary, parse failure |

All field names are **camelCase** in JSON. The `type` tag is **kebab-case**.
