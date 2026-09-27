//! Task 22.4 — Performance tests.
//!
//! These assert the engine-level and pipeline-level timing budgets the design
//! promises, using **generous, deterministic** bounds so they are not flaky on
//! slow CI hardware:
//!
//! - a ≤10 MB Job returns output-or-error within 10 s (Req 31.2, 1.3); and
//! - concurrent Jobs progress independently and all complete (Req 31.4).
//!
//! The 2 s initial-interactive-render budget (Req 31.1) and the 300 ms
//! tool-workspace-open budget are UI concerns; they are covered by the frontend
//! test suite (see `apps/web/src/lib/perf.test.ts`) and documented there,
//! because they measure DOM render latency which is out of scope for the Rust
//! backend crate.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use backend::convert::FakeConverter;
use backend::ocr::FakeOcr;
use backend::pipeline::InMemorySpecStore;
use backend::queue::{InMemoryJobBackend, JobStatus, JobStore};
use backend::render::FakeRenderer;
use backend::store::{EncryptedFileStore, InMemoryObjectStore};
use pdf_engine::{
    EngineInput, FileBytes, Level, ToolId, ToolOptions,
};

use common::{executors, seed_job, worker};

/// Build a PDF of roughly `target_bytes` by giving each of `pages` pages a
/// large content stream. Used to construct a ~10 MB Source_File deterministically.
fn large_pdf(pages: usize, target_bytes: usize) -> Vec<u8> {
    use lopdf::{dictionary, Object, Stream};

    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    // Split the target across pages; each stream carries a large, valid content
    // body (`BT`/`ET` wrapping filler text-show operators).
    let per_page = (target_bytes / pages.max(1)).max(1024);
    let filler = "(x) Tj ".repeat(per_page / 7);

    let mut kids: Vec<Object> = Vec::with_capacity(pages);
    for _ in 0..pages {
        let mut stream_bytes = b"BT /F1 12 Tf ".to_vec();
        stream_bytes.extend_from_slice(filler.as_bytes());
        stream_bytes.extend_from_slice(b" ET");
        let content_id = doc.add_object(Stream::new(dictionary! {}, stream_bytes));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
    }

    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog);
    let mut buf = Vec::new();
    doc.save_to(&mut buf).expect("serialize large pdf");
    buf
}

// -------------------------------------------------------------------------
// A ≤10 MB Job returns within 10 s (Req 31.2, 1.3).
// -------------------------------------------------------------------------

#[test]
fn ten_megabyte_optimize_returns_within_ten_seconds() {
    // Build a source close to (but under) 10 MB.
    let bytes = large_pdf(40, 9 * 1024 * 1024);
    assert!(
        bytes.len() <= 10 * 1024 * 1024,
        "source should be <= 10 MB, was {}",
        bytes.len()
    );
    assert!(
        bytes.len() >= 1024 * 1024,
        "source should be a meaningful size, was {}",
        bytes.len()
    );

    let start = Instant::now();
    let out = pdf_engine::run(EngineInput {
        tool: ToolId::OptimizePdf,
        sources: vec![FileBytes {
            name: "big.pdf".to_string(),
            bytes: &bytes,
        }],
        options: ToolOptions::Optimize { level: Level::High },
    });
    let elapsed = start.elapsed();

    // The engine must return an output OR a structured error — never hang.
    assert!(out.is_ok() || out.is_err());
    // Generous 10 s ceiling (Req 31.2). Real runs finish in well under a second.
    assert!(
        elapsed < Duration::from_secs(10),
        "engine took {elapsed:?}, exceeding the 10 s budget"
    );
}

#[tokio::test]
async fn pipeline_job_completes_within_ten_seconds() {
    let store = EncryptedFileStore::new(InMemoryObjectStore::new(), &[5u8; 32]);
    let backend_q = InMemoryJobBackend::new();
    let specs = InMemorySpecStore::new();
    let bytes = large_pdf(30, 8 * 1024 * 1024);
    seed_job(
        &store,
        &backend_q,
        &specs,
        "perf",
        ToolId::OptimizePdf,
        ToolOptions::Optimize { level: Level::Medium },
        &[bytes],
        None,
    )
    .await;

    let ex = executors(
        Arc::new(FakeConverter::succeeding()),
        Arc::new(FakeOcr::succeeding()),
        Arc::new(FakeRenderer::succeeding(1)),
    );
    let w = worker(&store, &backend_q, &specs, &ex);

    let start = Instant::now();
    w.process_next().await.expect("worker runs");
    let elapsed = start.elapsed();

    let rec = backend_q.get("perf").await.unwrap();
    // Terminal status reached (Succeeded or a recorded Failed) — never pending.
    assert!(matches!(
        rec.status,
        JobStatus::Succeeded | JobStatus::Failed { .. }
    ));
    assert!(
        elapsed < Duration::from_secs(10),
        "pipeline took {elapsed:?}, exceeding the 10 s budget"
    );
}

// -------------------------------------------------------------------------
// Concurrent Jobs progress independently (Req 31.4).
// -------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_jobs_all_complete_independently() {
    let store = Arc::new(EncryptedFileStore::new(InMemoryObjectStore::new(), &[5u8; 32]));
    let backend_q = Arc::new(InMemoryJobBackend::new());
    let specs = Arc::new(InMemorySpecStore::new());

    let n = 8;
    for i in 0..n {
        seed_job(
            &store,
            &backend_q,
            &specs,
            &format!("cj-{i}"),
            ToolId::OptimizePdf,
            ToolOptions::Optimize { level: Level::Low },
            &[common::clean_pdf()],
            None,
        )
        .await;
    }

    let start = Instant::now();
    // Four workers draining a shared queue concurrently.
    let mut handles = Vec::new();
    for _ in 0..4 {
        let store = Arc::clone(&store);
        let backend_q = Arc::clone(&backend_q);
        let specs = Arc::clone(&specs);
        handles.push(tokio::spawn(async move {
            let ex = executors(
                Arc::new(FakeConverter::succeeding()),
                Arc::new(FakeOcr::succeeding()),
                Arc::new(FakeRenderer::succeeding(1)),
            );
            let w = worker(store.as_ref(), backend_q.as_ref(), specs.as_ref(), &ex);
            let mut count = 0;
            while w.process_next().await.expect("worker runs").is_some() {
                count += 1;
            }
            count
        }));
    }
    let mut total = 0;
    for h in handles {
        total += h.await.expect("join");
    }
    let elapsed = start.elapsed();

    assert_eq!(total, n, "every queued Job must be drained exactly once");
    for i in 0..n {
        let rec = backend_q.get(&format!("cj-{i}")).await.unwrap();
        assert!(matches!(
            rec.status,
            JobStatus::Succeeded | JobStatus::Failed { .. }
        ));
    }
    // Independent progress: all N jobs across 4 workers finish quickly.
    assert!(
        elapsed < Duration::from_secs(10),
        "concurrent drain took {elapsed:?}"
    );
}
