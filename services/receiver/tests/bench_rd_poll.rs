use receiver::control_api::AppState;
use receiver::db::Db;
use receiver::rd_dbf::RD_FILES;
use receiver::rd_poll::{PollOutcome, poll_once};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rd")
}

fn copy_fixtures_to(dir: &Path) {
    for file in RD_FILES {
        std::fs::copy(fixtures_dir().join(file), dir.join(file)).unwrap();
    }
}

#[tokio::test]
async fn bench_rd_poll_signature() {
    let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
    let tmp = tempfile::tempdir().unwrap();
    copy_fixtures_to(tmp.path());

    let mut last = None;

    // Warm-up / initial import pass
    let first = poll_once(&state, tmp.path(), &mut last).await;
    assert!(matches!(first, PollOutcome::Imported(_)));

    let iterations = 1000;
    let start = Instant::now();

    for _ in 0..iterations {
        let outcome = poll_once(&state, tmp.path(), &mut last).await;
        assert!(matches!(outcome, PollOutcome::Skipped));
    }

    let elapsed = start.elapsed();
    let avg = elapsed / iterations;

    println!("\n==================================================");
    println!("BENCHMARK: rd_poll change detection (signature check):");
    println!("Total time for {iterations} iterations: {:?}", elapsed);
    println!("Average time per poll pass: {:?}", avg);
    println!("==================================================\n");
}
