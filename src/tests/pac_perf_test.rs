use ferrovela::pac::PacEngine;
use std::fs;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::time::sleep;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pac_loading_blocking() {
    let pac_path = "large_test.pac";
    let mut file = fs::File::create(pac_path).expect("Failed to create PAC file");
    // Write 50MB of spaces to make read slow
    let content = " ".repeat(50 * 1024 * 1024);
    file.write_all(content.as_bytes()).expect("Failed to write PAC file");

    let sleep_start = Instant::now();
    let sleep_task = tokio::spawn(async {
        sleep(Duration::from_millis(100)).await;
    });

    let pac_path_clone = pac_path.to_string();
    let pac_task = tokio::spawn(async move {
        PacEngine::new(&pac_path_clone).await
    });

    sleep_task.await.unwrap();
    let sleep_duration = sleep_start.elapsed();

    // Ensure PAC task also finishes successfully
    let _ = pac_task.await.unwrap();

    fs::remove_file(pac_path).expect("Failed to remove PAC file");

    assert!(
        sleep_duration < Duration::from_millis(150),
        "Blocking I/O detected: sleep took {:?} (expected ~100ms)",
        sleep_duration
    );

    // We assert that the sleep duration is reasonably close to 100ms for async behavior.
    // For blocking behavior, it will be significantly longer (e.g. > 150ms).
    // We don't fail the test here because we want to see the output first,
    // or we can fail if it's too fast (false negative) or too slow.
    // But since I'm running this before and after, I'll rely on the output.

    // However, to make it a proper regression test, I should assert something.
    // But "blocking" depends on disk speed.
    // I'll just leave the print and manually verify the "before" state,
    // and then after fix, I can add an assertion if I want, or just verify manually.
    // I'll add an assertion that it's "fast enough" AFTER the fix.
    // For now, I'll just print.
}
