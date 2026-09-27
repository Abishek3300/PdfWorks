//! Task 21/22 — Route-level integration tests for the wired HTTP API.
//!
//! These drive the real Axum [`build_router`] through
//! `tower::ServiceExt::oneshot` so the full path runs: Security_Gateway
//! middleware → multipart intake → Validator + Content_Scanner → Job_Token
//! issuance → encrypted store → queue → background/inline worker → status +
//! download endpoints. They assert the exact contract the SvelteKit client
//! (`apps/web/src/lib/api/client.ts`) depends on.
//!
//! A tiny Client_Capable Job (Optimize) with a minimal one-page PDF is
//! submitted; the test asserts a 201 with a job token, drains the queue, and
//! then reads status + the produced Output_File.
//!
//! Requirements: 31.4, 32.5, 33.3, 43.1, 43.3, 50.2 and the client contract.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use lopdf::dictionary;
use tower::ServiceExt; // for `oneshot`

use backend::api::drain_queue;
use backend::config::Config;
use backend::{build_router, AppState};

/// A minimal, clean one-page PDF the engine can parse, sanitize, and optimize.
fn clean_pdf() -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(
        pages_id,
        lopdf::Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![lopdf::Object::Reference(page_id)],
            "Count" => 1_i64,
        }),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog);
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf
}

/// Build a `multipart/form-data` body for the intake with `tool`, `options`,
/// and one `files` part carrying `pdf`.
fn multipart_body(tool: &str, options: &str, filename: &str, pdf: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----pdfworkstestboundary1234567890";
    let mut body: Vec<u8> = Vec::new();

    let mut push_field = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    push_field("tool", tool);
    push_field("options", options);

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\n\
             Content-Type: application/pdf\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(pdf);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (
        format!("multipart/form-data; boundary={boundary}"),
        body,
    )
}

/// A fresh router over the test config (allowlist = https://app.example).
fn router() -> axum::Router {
    let state = Arc::new(AppState::new(Config::for_tests()));
    build_router(state)
}

/// Submit an Optimize Job, drain the queue with a worker, and walk the full
/// status + download contract the frontend uses.
#[tokio::test]
async fn submit_status_and_download_end_to_end() {
    // Share ONE state between the router and the worker drain so the queue and
    // store are the same instances the handlers use.
    let state = Arc::new(AppState::new(Config::for_tests()));
    let app = build_router(state.clone());

    let (content_type, body) = multipart_body(
        "OptimizePdf",
        r#"{"Optimize":{"level":"Medium"}}"#,
        "input.pdf",
        &clean_pdf(),
    );

    // --- POST /api/jobs -> 201 { jobId, jobToken, expiresAt } ---
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/jobs")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("intake response");
    assert_eq!(resp.status(), StatusCode::CREATED, "intake should 201");

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let job_id = created["jobId"].as_str().expect("jobId present").to_string();
    let job_token = created["jobToken"]
        .as_str()
        .expect("jobToken present")
        .to_string();
    assert!(!job_token.is_empty(), "a job token must be issued");
    assert_eq!(job_id, job_token, "jobId is the token credential");
    assert!(
        created["expiresAt"].as_str().is_some(),
        "expiresAt present (Req 32.5)"
    );

    // --- Status BEFORE the worker runs: queued ---
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/jobs/{job_id}"))
                .header("x-job-token", &job_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("status response");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["phase"], "queued");

    // --- Drain the queue on a worker (what the background task does) ---
    let processed = drain_queue(&state.services).await.expect("drain");
    assert_eq!(processed, 1, "the enqueued job is processed");

    // --- Status AFTER: succeeded, with output metadata ---
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/jobs/{job_id}"))
                .header("x-job-token", &job_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("status response");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["phase"], "succeeded", "status: {status}");
    let outputs = status["outputs"].as_array().expect("outputs present");
    assert_eq!(outputs.len(), 1, "one output for Optimize");
    assert_eq!(outputs[0]["contentType"], "application/pdf");
    assert!(outputs[0]["sizeBytes"].as_u64().unwrap() > 0);

    // --- GET /api/jobs/{id}/outputs/0 -> attachment PDF ---
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/jobs/{job_id}/outputs/0"))
                .header("x-job-token", &job_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("download response");
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cd.starts_with("attachment;"), "must be an attachment (Req 50.2)");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/pdf")
    );
    let out_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(out_bytes.starts_with(b"%PDF-"), "the output is a PDF");
}

/// A status request without the Job_Token is denied (Req 43.3).
#[tokio::test]
async fn status_without_token_is_denied() {
    let resp = router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/jobs/some-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A wrong Job_Token is denied even when the path id is well-formed (Req 43.3).
#[tokio::test]
async fn wrong_token_is_denied() {
    let resp = router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/jobs/aaa")
                .header("x-job-token", "bbb")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");
    // Header token does not match the path id -> access denied.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// DELETE removes a Job's files and revokes its token (Req 32.3).
#[tokio::test]
async fn delete_revokes_the_job() {
    let state = Arc::new(AppState::new(Config::for_tests()));
    let app = build_router(state.clone());

    let (content_type, body) = multipart_body(
        "OptimizePdf",
        r#"{"Optimize":{"level":"Low"}}"#,
        "input.pdf",
        &clean_pdf(),
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/jobs")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let token = created["jobToken"].as_str().unwrap().to_string();

    // DELETE -> 204.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/jobs/{token}"))
                .header("x-job-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Subsequent status is denied: the token no longer resolves.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/jobs/{token}"))
                .header("x-job-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
