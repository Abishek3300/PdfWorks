# syntax=docker/dockerfile:1.7
# =============================================================================
# Multi-stage image for the PDF Tools Suite backend (Server_Side_Processing).
#
# Stage 1 builds the Rust Axum service (native target). Stage 2 is a slim
# runtime that also hosts the Server_Only conversion dependencies: LibreOffice
# headless (Office <-> PDF, HTML -> PDF), OCRmyPDF/Tesseract (Scan-to-PDF OCR
# text layer), and the PDF render/PDF/A tooling (Ghostscript for PDF/A, MuPDF's
# `mutool` for page rasterization). These are invoked by the backend as
# sandboxed subprocesses (crates/backend/src/{convert,ocr,render}.rs, Task 19).
#
# Every base image is pinned by tag + digest-friendly version (Req 45.1), and
# the image is scanned by the Dependency_Scanner in CI (Req 45.2/45.3).
# =============================================================================

# ----------------------------------------------------------------------------
# Stage 1: build
# ----------------------------------------------------------------------------
FROM rust:1.98.1-bookworm AS build
WORKDIR /app

# Pre-cache dependency compilation using only the manifests.
COPY Cargo.toml Cargo.lock ./
COPY crates/pdf-engine/Cargo.toml crates/pdf-engine/Cargo.toml
COPY crates/backend/Cargo.toml crates/backend/Cargo.toml
# Create dummy sources so `cargo build` resolves and compiles dependencies
# before the real source is copied (better layer caching).
RUN mkdir -p crates/pdf-engine/src crates/backend/src \
    && echo "pub fn run() {}" > crates/pdf-engine/src/lib.rs \
    && echo "fn main() {}" > crates/backend/src/main.rs \
    && cargo build --release -p backend || true

# Copy the real source and build the release binary.
COPY crates ./crates
RUN cargo build --release -p backend

# ----------------------------------------------------------------------------
# Stage 2: runtime
# ----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Minimal runtime deps; ca-certificates for outbound TLS (URL_Fetcher, S3).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# -----------------------------------------------------------------------------
# Server_Only conversion dependency layers (Task 19). Each is a sandboxed
# subprocess the backend shells out to; the Rust code (convert/ocr/render) hides
# these behind traits so unit tests need none of them installed.
#
# LibreOffice headless — Word/PowerPoint/Excel <-> PDF, HTML -> PDF, PDF -> Office
# (Req 13-16, 19-21). Invoked as `soffice --headless --convert-to` by
# crates/backend/src/convert.rs::SoffConverter.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libreoffice-core libreoffice-writer libreoffice-calc libreoffice-impress \
    && rm -rf /var/lib/apt/lists/*

# OCR — OCRmyPDF + Tesseract for the Scan-to-PDF OCR text layer (Req 9.2).
# Invoked as `ocrmypdf --skip-text` by crates/backend/src/ocr.rs::OcrMyPdfEngine.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ocrmypdf tesseract-ocr \
    && rm -rf /var/lib/apt/lists/*

# PDF render / PDF/A tooling (Req 8.1, 22). Ghostscript performs the PDF/A
# OutputIntent + font embedding (`gs -dPDFA`); MuPDF's `mutool draw` rasterizes
# pages for thumbnails / server-side PDF->JPG. A pdfium wrapper exposing the same
# CLI is an accepted drop-in. Invoked by crates/backend/src/render.rs::ExternalRenderer.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ghostscript mupdf-tools \
    && rm -rf /var/lib/apt/lists/*
# -----------------------------------------------------------------------------

# Run as a non-root user (defense in depth; sandbox hardening in Task 17).
RUN useradd --system --uid 10001 --create-home appuser
USER appuser
WORKDIR /home/appuser

COPY --from=build /app/target/release/backend /usr/local/bin/backend

ENV BACKEND_ADDR=0.0.0.0:8080
EXPOSE 8080

# Liveness endpoint served by the scaffold (src/main.rs).
HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD ["/usr/local/bin/backend", "--health-check"] || exit 1

ENTRYPOINT ["/usr/local/bin/backend"]
