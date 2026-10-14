use receiver::control_api::AppState;
use receiver::db::Db;
use receiver::rd_dbf::RD_FILES;
use receiver::rd_poll::poll_once;
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
#[ignore]
async fn bench_rd_poll_once() {
    let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
    let tmp = tempfile::tempdir().unwrap();
    copy_fixtures_to(tmp.path());

    // Warmup
    let mut last = None;
    poll_once(&state, tmp.path(), &mut last).await;

    let iterations = 100;
    let start = Instant::now();
    for _ in 0..iterations {
        let mut last = None; // Force snapshot_and_load each iteration
        poll_once(&state, tmp.path(), &mut last).await;
    }
    let elapsed = start.elapsed();
    let avg = elapsed / iterations;

    println!("\n==================================================");
    println!("BENCHMARK: poll_once snapshot_and_load ({iterations} iterations):");
    println!("Total time: {:?}", elapsed);
    println!("Average time per poll: {:?}", avg);
    println!("==================================================\n");
}
