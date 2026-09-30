// SPDX-License-Identifier: Apache-2.0
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn test_real_process_spawn_hook_enqueue_p95_under_100ms() {
    let bin = env!("CARGO_BIN_EXE_tokentree");
    assert!(
        fs::metadata(bin).is_ok(),
        "tokentree binary must exist at {bin}"
    );

    let temp = tempdir().unwrap();
    let sample_prompt = "Investigate memory usage regression in billing pipeline";
    let hook_payload = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "ses_bench_process_spawn",
        "prompt": sample_prompt,
        "cwd": "/workspace/project",
        "tool_input": { "content": "SELECT * FROM sensitive_tokens" }
    })
    .to_string();

    let iterations = 50;
    let mut latencies: Vec<Duration> = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();

        let mut child = Command::new(bin)
            .arg("--home")
            .arg(temp.path())
            .arg("hook-enqueue")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn tokentree hook-enqueue");

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(hook_payload.as_bytes())
                .expect("Failed to pipe hook payload to stdin");
        }

        let output = child
            .wait_with_output()
            .expect("Failed to wait for tokentree hook-enqueue");
        let elapsed = start.elapsed();

        assert!(
            output.status.success(),
            "tokentree hook-enqueue failed with stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        latencies.push(elapsed);
    }

    latencies.sort();

    let p50 = latencies[(iterations * 50) / 100];
    let p95 = latencies[(iterations * 95) / 100];
    let p99 = latencies[(iterations * 99) / 100];

    println!(
        "Process spawn hook-enqueue benchmark ({} iterations): p50={:?}, p95={:?}, p99={:?}",
        iterations, p50, p95, p99
    );

    // Verify spool file was created with secure append and sanitization
    let spool_file = temp.path().join("spool/claude-hooks.jsonl");
    assert!(spool_file.exists(), "Spool file must exist");
    let spool_content = fs::read_to_string(&spool_file).unwrap();
    let lines: Vec<&str> = spool_content.lines().collect();
    assert_eq!(
        lines.len(),
        iterations,
        "Spool must contain one line per iteration"
    );

    // Verify raw prompt and tool content were dropped by sanitizer
    for line in &lines {
        assert!(
            !line.contains(sample_prompt),
            "Sanitizer must drop raw prompt text from spool"
        );
        assert!(
            !line.contains("sensitive_tokens"),
            "Sanitizer must drop sensitive tool input from spool"
        );
    }

    // Assert strict p95 performance budget
    assert!(
        p95 < Duration::from_millis(100),
        "Real process spawn p95 latency {:?} exceeded 100ms budget (p50={:?}, p99={:?})",
        p95,
        p50,
        p99
    );
}
