//! Sandbox worker harness (Task 17.1, Req 40.1-40.7).
//!
//! Each Server_Side Job runs inside a Sandbox that:
//! - executes as a **non-root** process (Req 40.5),
//! - mounts the filesystem **read-only except a per-Job Scratch_Directory**
//!   (Req 40.6),
//! - has **no outbound network** (Req 40.2),
//! - is bounded by **CPU / memory / wall-clock** limits (Req 40.3), terminating
//!   with an error that names the exceeded resource (Req 40.4), and
//! - is **isolated from other Jobs' Scratch_Directories** (Req 40.7, Prop 22).
//!
//! ## Split of responsibilities (documented, per the task)
//!
//! The container runtime (see `docker/`) provides `network-none`, a non-root
//! user, a read-only root filesystem, and seccomp. On Linux the process launcher
//! additionally applies `setrlimit` (RLIMIT_AS / RLIMIT_CPU). This Rust harness
//! owns the parts that are enforceable in-process and are directly testable:
//! the **wall-clock + memory ceiling** around a unit of work and **per-Job
//! Scratch_Directory allocation + access control** so one Job's key space can
//! never reach another's (Property 22).

use std::path::{Path, PathBuf};
use std::time::Duration;

use pdf_engine::ResourceKind;

/// A resource-limit breach, naming the exceeded resource (Req 40.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitExceeded {
    /// Which bounded resource was exceeded.
    pub resource: ResourceKind,
}

impl LimitExceeded {
    /// A message that names the exceeded resource (Req 40.4).
    #[must_use]
    pub fn message(&self) -> String {
        let name = match self.resource {
            ResourceKind::Cpu => "CPU time",
            ResourceKind::Memory => "memory",
            ResourceKind::WallClock => "wall-clock time",
            ResourceKind::OutputSize => "output size",
        };
        format!("the job exceeded its {name} limit")
    }
}

/// A per-Job Scratch_Directory. Access is confined to paths *inside* this
/// directory; any attempt to resolve a path outside it is refused (Req 40.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScratchDir {
    /// Opaque Job identifier that owns this scratch space.
    job_token: String,
    /// Absolute root of this Job's scratch space.
    root: PathBuf,
}

impl ScratchDir {
    /// Allocate a scratch directory for `job_token` under `base`.
    ///
    /// The directory name is derived from the Job_Token so two distinct Jobs
    /// never share a root. The name is sanitized to a single safe path segment
    /// (reusing the engine's guarantee) so it cannot escape `base` (Req 50.1).
    #[must_use]
    pub fn allocate(base: &Path, job_token: &str) -> Self {
        let segment = pdf_engine::sanitize_filename(job_token);
        Self {
            job_token: job_token.to_string(),
            root: base.join(segment),
        }
    }

    /// The absolute root of this Job's scratch space.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The Job_Token that owns this scratch space.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.job_token
    }

    /// Resolve `relative` to an absolute path inside this scratch directory,
    /// refusing any component that would escape it (Req 40.7, Property 22).
    ///
    /// # Errors
    ///
    /// Returns [`ScratchError::Escapes`] when `relative` is absolute or contains
    /// a `..` component that would leave the scratch root.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, ScratchError> {
        let candidate = Path::new(relative);
        if candidate.is_absolute() {
            return Err(ScratchError::Escapes);
        }
        let mut depth: i32 = 0;
        for comp in candidate.components() {
            use std::path::Component;
            match comp {
                Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(ScratchError::Escapes);
                    }
                }
                Component::Normal(_) | Component::CurDir => depth += 0,
                Component::RootDir | Component::Prefix(_) => {
                    return Err(ScratchError::Escapes)
                }
            }
            if let Component::Normal(_) = comp {
                depth += 1;
            }
        }
        Ok(self.root.join(candidate))
    }

    /// Whether this Job may access `path`: only if `path` is within *its own*
    /// scratch root and not within another Job's root (Req 40.7).
    #[must_use]
    pub fn may_access(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }
}

/// Errors from scratch-directory path handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScratchError {
    /// The requested path would escape the Scratch_Directory (Req 40.7).
    Escapes,
}

/// A description of the observed resource usage of a unit of work, used by the
/// harness to decide whether a limit was breached. Supplied by the caller (the
/// dispatcher measures wall-clock; the OS/container measures memory/CPU), which
/// keeps the harness pure and unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Elapsed wall-clock time.
    pub wall_clock: Duration,
    /// Peak resident memory in bytes.
    pub peak_memory_bytes: u64,
    /// Consumed CPU time.
    pub cpu: Duration,
}

/// Enforce the sandbox limits against observed `usage`, returning the exceeded
/// resource (Req 40.3, 40.4). Wall-clock is checked first so a hang is reported
/// as a wall-clock breach even if it also blew the CPU budget.
///
/// # Errors
///
/// Returns [`LimitExceeded`] naming the first exceeded resource.
pub fn enforce_limits(
    usage: Usage,
    limits: &crate::config::SandboxLimits,
) -> Result<(), LimitExceeded> {
    if usage.wall_clock > limits.wall_clock {
        return Err(LimitExceeded {
            resource: ResourceKind::WallClock,
        });
    }
    if usage.peak_memory_bytes > limits.max_memory_bytes {
        return Err(LimitExceeded {
            resource: ResourceKind::Memory,
        });
    }
    if usage.cpu > limits.max_cpu {
        return Err(LimitExceeded {
            resource: ResourceKind::Cpu,
        });
    }
    Ok(())
}

/// Run a closure under a wall-clock ceiling, returning either its output or a
/// wall-clock [`LimitExceeded`] (Req 40.3, 40.4).
///
/// This is the in-process enforcement seam: it measures the elapsed time and
/// applies the limit. The container runtime additionally caps memory/CPU via
/// rlimit + cgroups; those breaches surface through [`enforce_limits`] with the
/// measured [`Usage`].
///
/// # Errors
///
/// Returns [`LimitExceeded`] with [`ResourceKind::WallClock`] when the closure
/// runs longer than `limit`.
pub fn with_wall_clock<T>(
    limit: Duration,
    work: impl FnOnce() -> T,
) -> Result<T, LimitExceeded> {
    let start = std::time::Instant::now();
    let out = work();
    if start.elapsed() > limit {
        return Err(LimitExceeded {
            resource: ResourceKind::WallClock,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SandboxLimits;

    fn base() -> PathBuf {
        PathBuf::from("/srv/scratch")
    }

    #[test]
    fn distinct_jobs_get_distinct_roots() {
        let a = ScratchDir::allocate(&base(), "job-A");
        let b = ScratchDir::allocate(&base(), "job-B");
        assert_ne!(a.root(), b.root());
    }

    #[test]
    fn resolve_stays_inside_scratch() {
        let s = ScratchDir::allocate(&base(), "job-A");
        let p = s.resolve("sub/output.pdf").unwrap();
        assert!(p.starts_with(s.root()));
    }

    #[test]
    fn resolve_rejects_traversal() {
        let s = ScratchDir::allocate(&base(), "job-A");
        assert_eq!(s.resolve("../job-B/secret").unwrap_err(), ScratchError::Escapes);
        assert_eq!(s.resolve("/etc/passwd").unwrap_err(), ScratchError::Escapes);
    }

    #[test]
    fn one_job_cannot_access_anothers_root() {
        let a = ScratchDir::allocate(&base(), "job-A");
        let b = ScratchDir::allocate(&base(), "job-B");
        // A path inside B's root is not accessible to A.
        let b_file = b.root().join("out.pdf");
        assert!(!a.may_access(&b_file));
        assert!(b.may_access(&b_file));
    }

    #[test]
    fn wall_clock_and_memory_and_cpu_named() {
        let limits = SandboxLimits {
            wall_clock: Duration::from_secs(1),
            max_memory_bytes: 100,
            max_cpu: Duration::from_secs(1),
        };
        assert_eq!(
            enforce_limits(
                Usage {
                    wall_clock: Duration::from_secs(2),
                    ..Usage::default()
                },
                &limits
            ),
            Err(LimitExceeded {
                resource: ResourceKind::WallClock
            })
        );
        assert_eq!(
            enforce_limits(
                Usage {
                    peak_memory_bytes: 200,
                    ..Usage::default()
                },
                &limits
            ),
            Err(LimitExceeded {
                resource: ResourceKind::Memory
            })
        );
        assert_eq!(
            enforce_limits(
                Usage {
                    cpu: Duration::from_secs(2),
                    ..Usage::default()
                },
                &limits
            ),
            Err(LimitExceeded {
                resource: ResourceKind::Cpu
            })
        );
    }

    #[test]
    fn within_limits_passes() {
        let limits = SandboxLimits::default();
        assert_eq!(
            enforce_limits(
                Usage {
                    wall_clock: Duration::from_millis(1),
                    peak_memory_bytes: 1024,
                    cpu: Duration::from_millis(1),
                },
                &limits
            ),
            Ok(())
        );
    }

    #[test]
    fn wall_clock_wrapper_runs_fast_work() {
        let out = with_wall_clock(Duration::from_secs(5), || 21 * 2).unwrap();
        assert_eq!(out, 42);
    }
}

// -------------------------------------------------------------------------
// Property 22: Sandboxed Jobs cannot access one another's scratch space.
// -------------------------------------------------------------------------
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        // Feature: pdf-tools-suite, Property 22: Sandboxed Jobs cannot access one another's scratch space.
        //
        // For all pairs of distinct Jobs, a Job process cannot read or write the
        // Scratch_Directory assigned to any other Job.
        // Validates: Requirements 40.7.
        #[test]
        fn jobs_cannot_reach_each_others_scratch(
            tok_a in "[A-Za-z0-9_-]{1,24}",
            tok_b in "[A-Za-z0-9_-]{1,24}",
            rel in "[A-Za-z0-9_./-]{1,40}",
        ) {
            prop_assume!(tok_a != tok_b);
            let base = std::path::PathBuf::from("/srv/scratch");
            let a = ScratchDir::allocate(&base, &tok_a);
            let b = ScratchDir::allocate(&base, &tok_b);

            // Distinct tokens that sanitize to the same segment would collide;
            // skip those (the token space is designed to be unique per Job).
            prop_assume!(a.root() != b.root());

            // Any path A can legally resolve stays within A's root and is thus
            // NOT accessible to B (Req 40.7).
            if let Ok(path) = a.resolve(&rel) {
                prop_assert!(a.may_access(&path));
                prop_assert!(!b.may_access(&path));
            }

            // A cannot resolve a path that lands in B's root.
            let escape = format!("../{}/x", pdf_engine::sanitize_filename(&tok_b));
            let resolved = a.resolve(&escape);
            if let Ok(p) = resolved {
                prop_assert!(!b.may_access(&p));
            } else {
                prop_assert_eq!(resolved.unwrap_err(), ScratchError::Escapes);
            }
        }
    }
}
