# Running & Testing Locally

## Prerequisites
- Docker (backend + services), Node 18+ (frontend), and ? to (re)generate the
  browser engine ? a Linux/CI environment with `wasm-bindgen-cli` 0.2.100.

## 1. Generate the WASM engine bundle (once, or after engine changes)
The shared Rust engine compiles to `wasm32`; the browser needs the wasm-bindgen
glue. Generate it (Linux/CI, or Docker on Windows):

```
docker run --rm -e CARGO_HOME=/usr/local/cargo -v "${PWD}:/w" -w /w rust:1.98.1-bookworm \
  bash -c "cargo install wasm-bindgen-cli --version 0.2.100 && \
  cargo build -p pdf-engine --target wasm32-unknown-unknown --release && \
  wasm-bindgen target/wasm32-unknown-unknown/release/pdf_engine.wasm \
    --target web --out-dir apps/web/src/lib/wasm --out-name pdf_engine"
```

This writes `apps/web/src/lib/wasm/pdf_engine.js` + `pdf_engine_bg.wasm`
(git-ignored build artifacts).

## 2. Run the backend
Full stack (backend + Redis + MinIO) once you have a pullable MinIO image:

```
docker compose up --build
```

Backend only (uses the in-memory encrypted File_Store fallback; fine for local
testing of every tool):

```
docker build -t pdf-tools-suite/backend:0.1.0 -f docker/backend.Dockerfile .
docker run -d --name pdfworks-backend -p 8080:8080 \
  -e CORS_ALLOWLIST=http://localhost:5173 \
  pdf-tools-suite/backend:0.1.0
```

Verify: `curl http://localhost:8080/healthz` -> `{"status":"ok"}`.

## 3. Run the frontend
```
cd apps/web
cp .env.example .env.local        # sets PUBLIC_API_BASE_URL=http://localhost:8080
npm install
npm run dev                        # http://localhost:5173
```

- Privacy Mode ON: tools run in the browser (WASM) ? no backend needed.
- Privacy Mode OFF / server-only tools: calls the backend on :8080.

## 4. Run the tests
```
# Engine (native + wasm) and frontend, on any host:
cargo +stable-x86_64-pc-windows-gnu test -p pdf-engine   # 45 tests
cd apps/web && npm run check && npm test                 # 20 tests, 0 errors

# Backend (Linux ? it links against system libs; use the container):
docker run --rm -e CARGO_HOME=/usr/local/cargo -v "${PWD}:/w" -w /w \
  rust:1.98.1-bookworm cargo test -p backend             # 150+ tests
```

## Production notes
- Set `BACKEND_PRODUCTION=true` and a real `FILE_STORE_KEY_HEX` (64 hex chars).
- Set `CORS_ALLOWLIST` to your real frontend origin(s).
- Terminate TLS at the reverse proxy (the app assumes HTTPS upstream).
- Frontend deploys to Cloudflare Pages; backend to a Docker host (Contabo/Fly.io);
  object storage to an S3-compatible bucket (R2).
