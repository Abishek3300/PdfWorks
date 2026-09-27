//! Task 22.5 — Sandbox isolation + resource-limit security suite.
//!
//! A consolidated suite over the public [`backend::sandbox`] surface covering
//! the design's sandbox security-test cases: one Job cannot reach another Job's
//! Scratch_Directory (Property 22), path traversal out of a scratch root is
//! refused, and a resource-limit breach terminates the Job with an error that
//! NAMES the exceeded resource (wall-clock / memory / CPU).
//!
//! Requirements: 40.2, 40.4, 40.5, 40.6, 40.7.
//!
//! The container runtime enforces the non-root user (40.5), read-only root FS
//! (40.6), and network-none (40.2) — documented in `docker/` and in the module.
//! The in-process, directly-testable guarantees (scratch isolation + limit
//! naming) are asserted here; the OS-enforced posture is documented at the
//! bottom.

use std::path::PathBuf;
use std::time::Duration;

use backend::config::SandboxLimits;
use backend::sandbox::{
    enforce_limits, with_wall_clock, LimitExceeded, ScratchDir, ScratchError, Usage,
};
use pdf_engine::ResourceKind;

fn base() -> PathBuf {
    PathBuf::from("/srv/scratch")
}

// -------------------------------------------------------------------------
// Suite: cross-Job scratch access is denied (Req 40.7, Property 22).
// -------------------------------------------------------------------------

#[test]
fn suite_distinct_jobs_get_distinct_roots() {
    let a = ScratchDir::allocate(&base(), "job-A");
    let b = ScratchDir::allocate(&base(), "job-B");
    assert_ne!(a.root(), b.root());
}

#[test]
fn suite_one_job_cannot_access_anothers_scratch() {
    let a = ScratchDir::allocate(&base(), "job-A");
    let b = ScratchDir::allocate(&base(), "job-B");

    // A file inside B's root is not accessible to A, but is to B (Req 40.7).
    let b_file = b.root().join("secret.pdf");
    assert!(!a.may_access(&b_file));
    assert!(b.may_access(&b_file));

    // A's own resolved paths stay inside A and are not accessible to B.
    let a_file = a.resolve("work/out.pdf").expect("resolve inside A");
    assert!(a.may_access(&a_file));
    assert!(!b.may_access(&a_file));
}

#[test]
fn suite_traversal_out_of_scratch_is_refused() {
    let a = ScratchDir::allocate(&base(), "job-A");
    // A relative `..` chain that would leave the root is refused (Req 40.7).
    assert_eq!(a.resolve("../job-B/steal").unwrap_err(), ScratchError::Escapes);
    // An absolute path is refused.
    assert_eq!(a.resolve("/etc/passwd").unwrap_err(), ScratchError::Escapes);
    // A benign nested relative path is allowed and stays inside the root.
    let p = a.resolve("a/b/c.pdf").expect("nested ok");
    assert!(p.starts_with(a.root()));
}

// -------------------------------------------------------------------------
// Suite: resource-limit termination names the exceeded resource (Req 40.4).
// -------------------------------------------------------------------------

#[test]
fn suite_wall_clock_breach_is_named() {
    let limits = SandboxLimits {
        wall_clock: Duration::from_secs(1),
        max_memory_bytes: 1024,
        max_cpu: Duration::from_secs(1),
    };
    let err = enforce_limits(
        Usage {
            wall_clock: Duration::from_secs(2),
            ..Usage::default()
        },
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.resource, ResourceKind::WallClock);
    assert!(err.message().contains("wall-clock time"), "msg: {}", err.message());
}

#[test]
fn suite_memory_breach_is_named() {
    let limits = SandboxLimits {
        wall_clock: Duration::from_secs(30),
        max_memory_bytes: 100,
        max_cpu: Duration::from_secs(30),
    };
    let err = enforce_limits(
        Usage {
            peak_memory_bytes: 200,
            ..Usage::default()
        },
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.resource, ResourceKind::Memory);
    assert!(err.message().contains("memory"), "msg: {}", err.message());
}

#[test]
fn suite_cpu_breach_is_named() {
    let limits = SandboxLimits {
        wall_clock: Duration::from_secs(30),
        max_memory_bytes: 1024 * 1024,
        max_cpu: Duration::from_secs(1),
    };
    let err = enforce_limits(
        Usage {
            cpu: Duration::from_secs(5),
            ..Usage::default()
        },
        &limits,
    )
    .unwrap_err();
    assert_eq!(err.resource, ResourceKind::Cpu);
    assert!(err.message().contains("CPU time"), "msg: {}", err.message());
}

#[test]
fn suite_within_limits_passes() {
    let limits = SandboxLimits::default();
    assert_eq!(
        enforce_limits(
            Usage {
                wall_clock: Duration::from_millis(5),
                peak_memory_bytes: 4096,
                cpu: Duration::from_millis(5),
            },
            &limits
        ),
        Ok(())
    );
}

#[test]
fn suite_wall_clock_wrapper_returns_work_output() {
    // A fast unit of work under a generous ceiling returns its output.
    let out = with_wall_clock(Duration::from_secs(5), || 6 * 7).expect("fast work ok");
    assert_eq!(out, 42);
}

#[test]
fn suite_limit_exceeded_messages_are_distinct_per_resource() {
    // Each resource produces a distinct, resource-naming message (Req 40.4).
    let msgs: Vec<String> = [
        ResourceKind::WallClock,
        ResourceKind::Memory,
        ResourceKind::Cpu,
        ResourceKind::OutputSize,
    ]
    .into_iter()
    .map(|resource| LimitExceeded { resource }.message())
    .collect();
    // All four are unique.
    for i in 0..msgs.len() {
        for j in (i + 1)..msgs.len() {
            assert_ne!(msgs[i], msgs[j], "messages {i} and {j} collide");
        }
    }
}

// -------------------------------------------------------------------------
// OS-enforced posture (documented; verified at the container layer).
// -------------------------------------------------------------------------
//
// The following sandbox guarantees are enforced by the container runtime, not
// this in-process harness, and are verified in the deployment image rather than
// a unit test:
//   - non-root process user               (Req 40.5)
//   - read-only root filesystem except the per-Job Scratch_Directory (Req 40.6)
//   - no outbound network (network-none)   (Req 40.2)
// They are configured in `docker/backend.Dockerfile` + the compose/run flags
// (`--network none`, `--read-only`, `--user`), and asserted by the CI job that
// launches the built image. This suite proves the parts that are enforceable
// and deterministic in-process (scratch isolation + resource-limit naming).
