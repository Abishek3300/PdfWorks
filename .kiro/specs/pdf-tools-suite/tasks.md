# Implementation Plan: PDF Tools Suite

## Overview

This plan implements the PDF Tools Suite as defined in the design: a monorepo with a
SvelteKit + TypeScript frontend, a Rust workspace whose shared `pdf-engine` crate compiles
to both `wasm32-unknown-unknown` (browser) and native (server), and an Axum backend service,
all packaged with Docker.

The shared `pdf-engine` crate is built first because both processing planes (Client_Side via
WASM in a Web Worker, and Server_Side via the native binary) depend on it, and because its
pure, I/O-free `run` function is the substrate for design Properties 1–13. Security helpers
(Properties 14–23) and the TypeScript registry/search property (Property 24) follow, then the
UI plane, the server plane, the Server_Only conversion tools, and finally cross-cutting
integration, performance, security, and accessibility/responsive test tasks.

Instruction followed: *Convert the feature design into a series of prompts for a
code-generation LLM that will implement each step with incremental progress. Each step builds
on the previous ones and ends by wiring things together, with no orphaned code. Tasks focus
only on writing, modifying, or testing code.*

## Tasks

- [x] 1. Scaffold the monorepo, Rust workspace, and Docker packaging
  - Create the repository layout: `apps/web` (SvelteKit + TypeScript), `crates/pdf-engine` (shared library), `crates/backend` (Axum service), and workspace-root `Cargo.toml`.
  - Configure the Rust workspace so `pdf-engine` builds for both `wasm32-unknown-unknown` and the native target; pin every crate to an exact version (Req 45.1).
  - Add a multi-stage `Dockerfile` for the backend image (Rust build + runtime) with placeholders for LibreOffice, OCR, and pdfium layers; add `docker-compose` wiring for the backend, Redis, and an S3-compatible store.
  - Add a CI/build script hook that runs a Dependency_Scanner over crates and container images and fails on critical-severity findings (Req 45.2, 45.3).
  - _Requirements: 45.1, 45.2, 45.3_

- [x] 2. Build the shared `pdf-engine` crate core surface and page-algebra tools
  - [x] 2.1 Define the engine's public API and data models
    - Implement `ToolId`, `EngineInput`, `EngineOutput` (including `source_page_counts`), `EngineError` (all variants), and the `ToolOptions` tagged union with its enums (`Level`, `Angle`, `Orientation`, `Margin`, etc.) exactly as in the design's Data Models.
    - Implement the single, I/O-free, panic-free `run(input) -> Result<EngineOutput, EngineError>` dispatch entry point with per-tool routing stubs.
    - _Requirements: 30.3_

  - [x] 2.2 Implement Merge, Split, Remove Pages, Extract Pages, and Organize
    - Implement Merge (concatenate in source order), Split (by split points and fixed-size ranges, rejecting out-of-range points with the actual page count), Remove Pages (complement in original order), Extract Pages (selection in original order), and Organize (permutation + per-page rotation + delete).
    - _Requirements: 4.1, 4.2, 5.1, 5.2, 5.3, 6.1, 7.1, 8.2, 8.3, 8.4, 30.3_

  - [x]* 2.3 Write property test for Merge page concatenation
    - **Property 1: Merge preserves and concatenates pages in source order**
    - **Validates: Requirements 4.1, 4.2**

  - [x]* 2.4 Write property test for Split partitioning
    - **Property 2: Split partitions the document exactly**
    - **Validates: Requirements 5.1, 5.2**

  - [x]* 2.5 Write property test for out-of-range split rejection
    - **Property 3: Split rejects out-of-range split points**
    - **Validates: Requirements 5.3**

  - [x]* 2.6 Write property test for Remove Pages complement
    - **Property 4: Remove Pages yields the complement in original order**
    - **Validates: Requirements 6.1**

  - [x]* 2.7 Write property test for Extract Pages count and order
    - **Property 5: Extract Pages page count equals selection size**
    - **Validates: Requirements 7.1, 30.3**

  - [x]* 2.8 Write property test for Organize permutation
    - **Property 6: Organize applies the requested permutation**
    - **Validates: Requirements 8.2, 8.3, 8.4**

- [x] 3. Implement size/quality, image, and Markdown engine tools
  - [x] 3.1 Implement Optimize and Compress
    - Implement both at Low/Medium/High levels, guaranteeing output size ≤ source size and unchanged page count.
    - _Requirements: 10.1, 10.3, 11.1, 11.3_

  - [x]* 3.2 Write property test for Optimize/Compress invariants
    - **Property 7: Optimize and Compress never grow the file and preserve page count**
    - **Validates: Requirements 10.1, 10.3, 11.1, 11.3**

  - [x] 3.3 Implement JPG to PDF and PDF to JPG
    - Implement JPG to PDF (one page per image, with orientation/margin/order options) and PDF to JPG via pdfium (`pdfium-render`) producing one JPG per page at the requested DPI.
    - _Requirements: 12.1, 12.2, 12.3, 18.1, 18.2_

  - [x]* 3.4 Write property test for JPG to PDF page count
    - **Property 8: JPG to PDF produces one page per image**
    - **Validates: Requirements 12.1**

  - [x]* 3.5 Write property test for PDF to JPG image count
    - **Property 9: PDF to JPG produces one image per page**
    - **Validates: Requirements 18.1**

  - [x]* 3.6 Write property test for JPG round-trip count preservation
    - **Property 10: JPG round-trip preserves image count**
    - **Validates: Requirements 30.2**

  - [x] 3.7 Implement Markdown to PDF and PDF to Markdown
    - Implement Markdown to PDF (render headings, lists, code blocks, tables, links) and PDF to Markdown (extract text, represent headings and lists as Markdown syntax), rejecting with `NoTextFound` where no text exists.
    - _Requirements: 17.1, 17.3, 23.1, 23.2, 23.3_

  - [x]* 3.8 Write property test for Markdown round-trip structure
    - **Property 11: Markdown round-trip preserves structure**
    - **Validates: Requirements 30.1**

- [x] 4. Implement rotation and per-page annotation engine tools
  - [x] 4.1 Implement Rotate, Add Page Numbers, Add Watermark, and Crop
    - Implement Rotate (90/180/270 on a page scope), Add Page Numbers (position + start number), Add Watermark (text/image, opacity, rotation), and Crop (region, optionally all pages), each preserving page count and marking every targeted page.
    - _Requirements: 24.1, 24.2, 24.3, 25.1, 25.2, 25.3, 26.1, 26.2, 26.3, 26.4, 27.1, 27.3_

  - [x]* 4.2 Write property test for rotation correctness and 4×90° identity
    - **Property 12: Rotate is angle-correct and four 90° turns are the identity**
    - **Validates: Requirements 24.1, 24.2, 24.3**

  - [x]* 4.3 Write property test for per-page annotation invariants
    - **Property 13: Per-page annotations preserve page count and mark every page**
    - **Validates: Requirements 25.1, 26.1, 27.1**

  - [x] 4.4 Implement Edit PDF and PDF Forms element application
    - Implement Edit PDF (apply added text/image/shape elements at their positions) and PDF Forms (write field values and include added interactive fields) in the engine.
    - _Requirements: 28.4, 29.2, 29.4_

- [x] 5. Checkpoint - engine core complete
  - Ensure all tests pass, ask the user if questions arise.

- [x] 6. Implement shared security helpers with property coverage
  - [x] 6.1 Implement filename sanitization
    - Implement a sanitizer that strips path separators and relative segments (e.g. `../`) so a display name resolves only inside the File_Store directory; assign unique names to colliding outputs.
    - _Requirements: 50.1, 49.3_

  - [x]* 6.2 Write property test for unique output names
    - **Property 14: Multiple outputs receive unique names**
    - **Validates: Requirements 49.3**

  - [x]* 6.3 Write property test for path-traversal-safe filenames
    - **Property 15: Filenames cannot cause path traversal**
    - **Validates: Requirements 50.1**

  - [x] 6.4 Implement the PDF Content_Scanner active-content handling
    - Implement detection plus removal-or-rejection of embedded JavaScript, launch actions, and embedded executables in PDF sources.
    - _Requirements: 39.5, 39.6_

  - [x]* 6.5 Write property test for active-content removal or rejection
    - **Property 16: Active content is removed or the file is rejected**
    - **Validates: Requirements 39.5, 39.6**

  - [x] 6.6 Implement the Job_Token generator and access checker
    - Generate URL-safe tokens with ≥128 bits of CSPRNG entropy; implement an access check that grants access iff the correct, unexpired token is presented.
    - _Requirements: 43.1, 43.2, 43.3, 43.4, 32.4, 36.6_

  - [x]* 6.7 Write property test for Job_Token entropy and uniqueness
    - **Property 19: Job_Tokens are high-entropy and unique**
    - **Validates: Requirements 43.1, 43.2**

  - [x]* 6.8 Write property test for token-gated, expiry-bound access
    - **Property 20: File access requires the correct, unexpired Job_Token**
    - **Validates: Requirements 43.3, 43.4, 32.4, 36.6**

- [x] 7. Compile the engine to WASM and integrate the client processing plane
  - [x] 7.1 Add the WASM binding layer and build the WASM bundle
    - Add `wasm-bindgen` bindings exposing `run` to JavaScript; produce the WASM bundle consumed by the frontend build.
    - _Requirements: 37.5, 47.1_

  - [x] 7.2 Implement the Web Worker host for on-device processing
    - Implement a Web Worker that loads the WASM engine and processes `postMessage(bytes, options)`, returning Output_File bytes to the main thread without any network transmission.
    - _Requirements: 47.1, 47.2, 32.6_

  - [x]* 7.3 Write property test for no source-byte transmission in Client_Side_Processing
    - **Property 21: Client_Side_Processing transmits no source bytes**
    - **Validates: Requirements 32.6, 47.2, 50.5**

- [x] 8. Implement the shared tool registry and TypeScript search
  - [x] 8.1 Define the tool registry
    - Implement the declarative `ToolDescriptor` table (id, category, label, capability, supportedFormats, minSources, producesMultiple) shared in shape between the TypeScript UI and the Rust backend; mark Server_Only tools per the design.
    - _Requirements: 1.1, 1.2, 1.6, 2.8, 33.1, 37.4, 37.7, 39.1_

  - [x] 8.2 Implement label substring search filtering
    - Implement the search function that returns exactly the Tools whose labels contain the query text.
    - _Requirements: 1.4, 1.5_

  - [x]* 8.3 Write property test for search filtering (fast-check)
    - **Property 24: Tool search filters by name substring**
    - **Validates: Requirements 1.5**

- [x] 9. Build the SvelteKit UI shell, navigation, and Privacy Mode routing
  - [x] 9.1 Implement the app shell, category navigation, and tool catalog
    - Render the five Tool_Categories with their Tools and descriptive labels, wire the search control, and open a tool workspace on selection (target ≤300 ms).
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6_

  - [x] 9.2 Implement the Privacy Mode state machine and on-device indicator
    - Implement the toggle (default disabled), derive Processing_Mode from capability + Privacy_Mode, disable the toggle with a server-processing message for Server_Only tools, disable Privacy Mode when WebAssembly is absent, prompt for confirmed Server_Side fallback when a file exceeds the client memory budget, and show the on-device indicator during client processing.
    - _Requirements: 37.1, 37.2, 37.3, 37.4, 37.6, 37.7, 37.8, 48.2_

- [x] 10. Implement upload, download, and per-tool option panels
  - [x] 10.1 Implement the Upload_Manager
    - Accept files via drag-and-drop and file dialog; enforce Max_File_Size and Max_Batch_Count client-side with clear messages; validate Supported_Format against the registry; for Server_Side show byte-percentage progress and offer retry on interruption; for Client_Side transmit nothing.
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 2.7, 2.8_

  - [x] 10.2 Implement the Download_Manager
    - Show each Output_File name and size pre-download; single output → direct download, multiple → ZIP plus per-file controls; no user authentication; allow retry of interrupted downloads within Retention_Period; display Retention_Period before upload for Server_Side.
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 18.3, 32.5, 48.4_

  - [x] 10.3 Implement per-tool option panels and result displays
    - Implement option UIs for all tools (merge reorder, split points/fixed-size, page selection by item and range, organize thumbnails with rotate/delete, optimize/compress level with size/reduction display, JPG orientation/margin/order, PDF→JPG DPI, rotate scope, page-number position/start, watermark text/image/opacity/rotation, crop visual region, edit element add/move/delete, form field editing/add, Markdown text input, HTML URL/orientation, PDF/A level, Scan-to-PDF reorder), and Scan-to-PDF camera capture with upload fallback.
    - _Requirements: 4.3, 5.2, 6.2, 7.2, 8.1, 8.3, 8.4, 9.1, 9.3, 9.4, 10.2, 10.4, 11.2, 11.4, 12.2, 12.3, 12.4, 16.2, 16.3, 17.2, 18.2, 22.2, 24.2, 24.3, 25.2, 25.3, 26.2, 26.3, 26.4, 27.2, 27.3, 28.1, 28.2, 28.3, 28.5, 28.6, 29.1, 29.3_

- [x] 11. Wire client-side job execution, chaining, and interruption handling
  - [x] 11.1 Wire tool workspaces to client-side execution and progress
    - Route Client_Side jobs through the Web Worker, display a processing progress indicator, and render outputs into the Download_Manager; sanitize any content derived from sources or untrusted input before rendering.
    - _Requirements: 31.3, 47.3_

  - [x] 11.2 Implement chained operations
    - Offer "continue to another Tool" on success, load selected Output_Files as next Source_Files without re-upload, re-run upload validation on carried files, allow selecting which outputs to carry, and keep files on-device across client-to-client chains under Privacy Mode.
    - _Requirements: 36.1, 36.2, 36.3, 36.4, 50.5_

  - [x] 11.3 Implement empty-state and multi-job/interruption UI handling
    - Show the Empty_State with run disabled when no file is present, warn on tab close/reload during a running Server_Side job, and display each running job's status independently.
    - _Requirements: 48.1, 48.3, 48.5_

- [x] 12. Implement responsive layout and accessibility
  - [x] 12.1 Implement responsive layouts
    - Multi-column at ≥1024 px, single-column at <768 px, touch-operable controls, and no horizontal scroll at ≥320 px.
    - _Requirements: 34.1, 34.2, 34.3, 34.4_

  - [x] 12.2 Implement accessibility features
    - Keyboard operability for all controls, text alternatives for non-text controls, WCAG AA contrast, and assistive-technology status announcements.
    - _Requirements: 35.1, 35.2, 35.3, 35.4, 35.5_

- [x] 13. Checkpoint - client plane complete
  - Ensure all tests pass, ask the user if questions arise.

- [x] 14. Implement the Security_Gateway
  - [x] 14.1 Implement TLS, security headers, CORS, body limit, and rate limiting
    - Terminate TLS 1.2+; attach HSTS (max-age ≥ 31536000), CSP without inline script, X-Content-Type-Options nosniff, frame-ancestors/X-Frame-Options, Referrer-Policy, and secure cookie attributes; enforce the CORS allowlist, a maximum request body size, and the Rate_Limiter for submissions and uploads (backed by Redis counters) returning a rate-limit status when exceeded.
    - _Requirements: 38.1, 38.2, 38.3, 38.4, 38.5, 38.6, 38.7, 42.1, 42.2, 42.3, 42.4, 50.3_

- [x] 15. Implement the Axum API intake, validation, and scanning
  - [x] 15.1 Implement Job intake, Job_Token issuance, and Redis queue
    - Implement Job creation issuing a Job_Token via the shared helper, persist Job state in Redis, and dispatch to workers so concurrent Jobs never block one another.
    - _Requirements: 31.4, 43.1, 43.2_

  - [x] 15.2 Implement the Validator
    - Implement content-signature type detection matching the tool's Supported_Formats, backend Max_File_Size and Max_Batch_Count enforcement, zero-byte and corrupt/malformed rejection, decompression-ratio limits, and archive (DOCX/XLSX/PPTX) uncompressed-size and nesting-depth limits.
    - _Requirements: 33.1, 33.2, 39.1, 39.2, 39.3, 39.4, 39.7, 42.5, 49.2, 50.4_

  - [x]* 15.3 Write property test for archive-bomb rejection
    - **Property 23: Archive-bomb inputs are rejected**
    - **Validates: Requirements 42.5, 50.4**

  - [x] 15.4 Wire the Content_Scanner into intake
    - Invoke the shared active-content scanner on uploaded PDFs during intake, stripping or rejecting embedded JavaScript, launch actions, and executables before enqueue.
    - _Requirements: 39.5, 39.6_

  - [x] 15.5 Implement the URL_Fetcher with SSRF guards
    - Fetch only http/https URLs, block Private_IP_Range addresses at the initial address and every redirect hop, cap redirects, and enforce a fetch timeout, rejecting the Job with the appropriate message on violation.
    - _Requirements: 16.4, 41.1, 41.2, 41.3, 41.4, 41.5, 41.6_

  - [x]* 15.6 Write property test for private-range rejection
    - **Property 17: URL fetches to private ranges are rejected**
    - **Validates: Requirements 41.3, 41.4**

  - [x]* 15.7 Write property test for URL scheme allowlist
    - **Property 18: Only http/https URL schemes are accepted**
    - **Validates: Requirements 41.1, 41.2**

- [x] 16. Implement File_Store, Retention_Service, and access control
  - [x] 16.1 Implement the encrypted File_Store addressed by Job_Token
    - Store Source_Files and Output_Files encrypted at rest, addressed only by opaque store keys scoped to the Job_Token, reject any directory-listing request, and set Content-Disposition attachment plus a non-executable Content-Type on downloads.
    - _Requirements: 43.3, 43.4, 43.5, 44.1, 50.2_

  - [x] 16.2 Implement the Retention_Service
    - Securely delete a Job's files when Retention_Period elapses and immediately on user request, via an S3 lifecycle rule plus an application-level sweeper; retain no partial output on failure.
    - _Requirements: 32.2, 32.3, 33.4, 44.2, 44.3_

- [x] 17. Implement sandboxed server execution and dispatch
  - [x] 17.1 Implement the Sandbox worker harness
    - Run each Server_Side Job as a non-root process with a read-only filesystem except its per-Job Scratch_Directory, no outbound network, CPU/memory/wall-clock limits, isolation from other Jobs' Scratch_Directories, and resource-limit termination that names the exceeded resource.
    - _Requirements: 40.1, 40.2, 40.3, 40.4, 40.5, 40.6, 40.7_

  - [x]* 17.2 Write property test for cross-Job scratch isolation
    - **Property 22: Sandboxed Jobs cannot access one another's scratch space**
    - **Validates: Requirements 40.7**

  - [x] 17.3 Wire the native engine dispatch and error surfacing
    - Dispatch Client_Capable tool work to the native `pdf-engine` inside the Sandbox, transmit files over encrypted connections, and surface failures with messages identifying the failed Tool and reason, including protected-file, no-output, and storage-exhaustion cases.
    - _Requirements: 31.2, 32.1, 33.3, 49.1, 49.4, 49.6_

- [x] 18. Checkpoint - server core complete
  - Ensure all tests pass, ask the user if questions arise.

- [x] 19. Implement Server_Only conversion tools
  - [x] 19.1 Integrate LibreOffice headless for Office↔PDF conversions
    - Wire sandboxed LibreOffice for Word to PDF (DOC/DOCX), PowerPoint to PDF (PPT/PPTX, one page per slide), Excel to PDF (XLS/XLSX, orientation), HTML to PDF (via the URL_Fetcher/inline), PDF to Word (DOCX, reading order preserved), PDF to PowerPoint (PPTX, one slide per page), and PDF to Excel (XLSX, one row per table row), rejecting with the appropriate messages on unrenderable input, no text, no table, or dependency unavailability.
    - _Requirements: 13.1, 13.2, 13.3, 14.1, 14.2, 14.3, 15.1, 15.2, 15.3, 15.4, 16.1, 19.1, 19.2, 19.3, 20.1, 20.2, 21.1, 21.2, 21.3, 49.5_

  - [x] 19.2 Integrate OCRmyPDF/Tesseract for Scan to PDF with OCR
    - Assemble provided images into a PDF (one page per image) and add an OCR text layer when requested, rejecting with a dependency-unavailable message when OCR is down.
    - _Requirements: 9.2, 49.5_

  - [x] 19.3 Integrate pdfium for PDF/A conversion and rendering
    - Implement PDF to PDF/A (A-1b/A-2b/A-3b, embedding referenced fonts) and provide page thumbnail/render output used by the Organize and preview UIs.
    - _Requirements: 8.1, 22.1, 22.2, 22.3_

- [x] 20. Implement Security_Log and anomaly recording
  - [x] 20.1 Implement privacy-preserving security logging
    - Record security-relevant events excluding PII and file content, restrict read access to authorized operators, and record an anomaly event when a client's rejected-request rate exceeds the threshold.
    - _Requirements: 44.4, 46.1, 46.2, 46.3, 46.4_

- [x] 21. Wire the end-to-end server flow
  - [x] 21.1 Connect gateway → API → queue → sandbox → store → download
    - Wire the full Server_Side path so an upload passes the Security_Gateway, is validated and scanned, enqueued, executed in a Sandbox by the engine or a Server_Only tool, stored encrypted under its Job_Token, and returned through the Download_Manager, with progress/status exposed to the UI.
    - _Requirements: 3.1, 31.2, 31.4_

- [x] 22. Cross-cutting integration, performance, security, and accessibility tests
  - [x]* 22.1 Write integration tests for Server_Only conversions
    - Exercise sandboxed LibreOffice/OCR/pdfium with 1–3 representative inputs each, including failure-message cases.
    - _Requirements: 13.1, 14.1, 15.1, 19.1, 20.1, 21.1, 22.1, 9.2, 13.3, 14.3, 15.4, 49.5_

  - [x]* 22.2 Write integration tests for security headers, CORS, and downloads
    - Assert HSTS, CSP (no inline script), X-Content-Type-Options, frame-ancestors/X-Frame-Options, Referrer-Policy, secure cookie attributes, CORS allowlist, and Content-Disposition/Content-Type on downloads.
    - _Requirements: 38.1, 38.2, 38.3, 38.4, 38.5, 38.6, 38.7, 50.2, 50.3_

  - [x]* 22.3 Write retention and access-control integration tests
    - Verify deletion at Retention_Period expiry and on demand, and Job_Token rejection after expiry and for directory-listing attempts.
    - _Requirements: 32.2, 32.3, 43.4, 43.5, 44.2, 44.3_

  - [x]* 22.4 Write performance tests
    - Assert initial interactive render within 2 s on broadband, a ≤10 MB Job returns output or error within 10 s, tool workspace opens within 300 ms, and concurrent Jobs progress independently.
    - _Requirements: 31.1, 31.2, 1.3, 31.4_

  - [x]* 22.5 Write the SSRF and sandbox security test suites
    - Cover scheme rejection, Private_IP_Range blocking on initial and redirect hops, redirect cap, and fetch timeout; cross-Job scratch access denial, no outbound network, non-root, read-only FS, and resource-limit termination.
    - _Requirements: 41.1, 41.2, 41.3, 41.4, 41.5, 41.6, 40.2, 40.4, 40.5, 40.6, 40.7_

  - [x]* 22.6 Write accessibility and responsive tests
    - Run automated `axe-core` WCAG AA checks (keyboard operability, text alternatives, contrast, status announcements) and viewport example tests at 320/768/1024 px for column layout and no horizontal scroll. Note: full WCAG AA conformance also requires manual assistive-technology testing and expert review.
    - _Requirements: 35.1, 35.2, 35.3, 35.4, 35.5, 34.1, 34.2, 34.4_

- [x] 23. Final checkpoint - ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional test sub-tasks and can be skipped for a faster MVP; core implementation tasks are never optional.
- Each task references specific requirement clauses for traceability, and each property test references its design property number.
- The shared `pdf-engine` (tasks 2–6) is implemented before both planes because Properties 1–13 and the security Properties 14–20 are proven once against the shared crate and hold for both the WASM and native builds.
- Property tests use `proptest` (Rust engine and security helpers) and `fast-check` (TypeScript registry/search), each running a minimum of 100 iterations, tagged `// Feature: pdf-tools-suite, Property {n}: ...`.
- Checkpoints (tasks 5, 13, 18, 23) provide incremental validation at plane boundaries.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["2.1"] },
    { "id": 2, "tasks": ["2.2", "3.1", "3.3", "3.7", "4.1", "4.4", "6.1", "6.4", "6.6", "8.1"] },
    { "id": 3, "tasks": ["2.3", "2.4", "2.5", "2.6", "2.7", "2.8", "3.2", "3.4", "3.5", "3.6", "3.8", "4.2", "4.3", "6.2", "6.3", "6.5", "6.7", "6.8", "7.1", "8.2"] },
    { "id": 4, "tasks": ["7.2", "8.3", "9.1", "9.2"] },
    { "id": 5, "tasks": ["7.3", "10.1", "10.2", "10.3"] },
    { "id": 6, "tasks": ["11.1", "11.2", "11.3", "12.1", "12.2"] },
    { "id": 7, "tasks": ["14.1", "15.1", "16.1"] },
    { "id": 8, "tasks": ["15.2", "15.4", "15.5", "16.2", "17.1"] },
    { "id": 9, "tasks": ["15.3", "15.6", "15.7", "17.2", "17.3", "20.1"] },
    { "id": 10, "tasks": ["19.1", "19.2", "19.3"] },
    { "id": 11, "tasks": ["21.1"] },
    { "id": 12, "tasks": ["22.1", "22.2", "22.3", "22.4", "22.5", "22.6"] }
  ]
}
```
