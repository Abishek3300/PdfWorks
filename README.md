# PDF Tools Suite

A privacy-first PDF toolkit. A shared Rust engine compiles to **WebAssembly**
(runs on-device in the browser) and **native** (runs server-side), so the same
logic backs both processing planes.

## Repository layout

```
.
├── apps/
│   └── web/               # SvelteKit + TypeScript frontend (static SPA)
├── crates/
│   ├── pdf-engine/        # Shared, I/O-free engine (wasm32 + native)
│   └── backend/           # Axum backend service (Server_Side_Processing)
├── docker/
│   └── backend.Dockerfile # Multi-stage backend image
├── scripts/
│   └── dependency-scan.sh # Dependency_Scanner (cargo audit + npm audit + trivy)
├── .github/workflows/     # CI: build (native + wasm), test, scan
├── docker-compose.yml     # backend + redis + minio (S3-compatible)
├── Cargo.toml             # Rust workspace (pinned dependency versions)
└── rust-toolchain.toml    # Pinned toolchain (1.98.1) + wasm32 target
```

## Prerequisites

- Rust 1.98.1 (`rust-toolchain.toml` pins this; `wasm32-unknown-unknown` target)
- Node 18.18+, npm 10+
- Docker (for the backend image and docker-compose)

## Build & verify

```bash
# Native workspace build (backend + engine)
cargo build

# WASM build of the shared engine (browser plane)
cargo build --target wasm32-unknown-unknown -p pdf-engine

# Tests
cargo test

# Frontend
cd apps/web
npm install
npm run check
npm run build
```

### Windows note

The workspace builds cleanly with the MSVC toolchain when the Visual Studio C++
Build Tools are installed (they provide `link.exe`). If only the rustup GNU host
is available, install a 64-bit **mingw-w64** toolchain (so `dlltool.exe` is on
PATH) and build with `cargo +stable-x86_64-pc-windows-gnu build`. The shared
`pdf-engine` crate has no native/system dependencies and builds for both the
native and `wasm32-unknown-unknown` targets on either toolchain. The canonical
build environment is Linux (see the Dockerfile and CI), where no extra linker
setup is required.

## Local stack

```bash
docker compose up --build
```

Brings up the backend (`:8080`), Redis, and MinIO (`:9000` API, `:9001` console).

## Security & supply chain

- Every dependency is pinned to an exact version (Req 45.1).
- CI runs the **Dependency_Scanner** (`scripts/dependency-scan.sh`) over the
  crates and the container image and fails the build on any critical-severity
  finding (Req 45.2, 45.3).
