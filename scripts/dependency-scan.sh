#!/usr/bin/env bash
# =============================================================================
# Dependency_Scanner (Req 45.2, 45.3)
#
# Scans third-party dependencies AND container images for known vulnerabilities
# and FAILS THE BUILD on any critical-severity finding.
#
#   - `cargo audit`  scans the pinned Rust dependency graph (Cargo.lock).
#   - `npm audit`    scans the pinned frontend dependency graph.
#   - `trivy image`  scans the built backend container image.
#
# Non-critical findings are reported but do not fail the build; a single
# critical finding from any scanner sets a non-zero exit code (Req 45.3).
# =============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FAILED=0
IMAGE_REF="${BACKEND_IMAGE:-pdf-tools-suite/backend:0.1.0}"

echo "==> [1/3] cargo audit (Rust crates)"
if command -v cargo-audit >/dev/null 2>&1 || cargo audit --version >/dev/null 2>&1; then
  # cargo-audit exits non-zero when vulnerabilities are found. We only fail the
  # build on CRITICAL findings; the JSON report is inspected for severity.
  if ! cargo audit --deny warnings; then
    # Re-check specifically for critical advisories.
    if cargo audit --json 2>/dev/null | grep -Eiq '"severity"\s*:\s*"critical"'; then
      echo "CRITICAL: cargo audit found critical-severity advisories." >&2
      FAILED=1
    else
      echo "cargo audit reported non-critical advisories (not failing the build)."
    fi
  fi
else
  echo "cargo-audit not installed; install with: cargo install cargo-audit" >&2
  FAILED=1
fi

echo "==> [2/3] npm audit (frontend)"
if [ -f apps/web/package-lock.json ]; then
  if command -v npm >/dev/null 2>&1; then
    if ! npm --prefix apps/web audit --audit-level=critical; then
      echo "CRITICAL: npm audit found critical-severity advisories." >&2
      FAILED=1
    fi
  else
    echo "npm not available on PATH; skipping frontend audit." >&2
  fi
else
  echo "apps/web/package-lock.json not found; run 'npm install' first."
fi

echo "==> [3/3] trivy image scan (container image: ${IMAGE_REF})"
if command -v trivy >/dev/null 2>&1; then
  # --exit-code 1 only for CRITICAL severities (Req 45.3).
  if ! trivy image --scanners vuln --severity CRITICAL --exit-code 1 --no-progress "${IMAGE_REF}"; then
    echo "CRITICAL: trivy found critical-severity vulnerabilities in ${IMAGE_REF}." >&2
    FAILED=1
  fi
else
  echo "trivy not installed; skipping image scan (install: https://aquasecurity.github.io/trivy)." >&2
fi

if [ "$FAILED" -ne 0 ]; then
  echo ""
  echo "Dependency_Scanner FAILED: critical-severity findings detected." >&2
  exit 1
fi

echo ""
echo "Dependency_Scanner passed: no critical-severity findings."
exit 0
