use receiver::db::Db;
use std::time::Instant;

fn main() {
    let mut db = Db::open_in_memory().expect("open memory db");

    // Generate 10,000 chip entries
    let count = 10_000;
    let chips: Vec<(i64, String)> = (1..=count)
        .map(|i| (i as i64, format!("CHIP_{:08X}", i)))
        .collect();

    // Warmup
    db.replace_bib_chips(&chips[..100]).unwrap();

    let iterations = 10;
    let mut total_duration = std::time::Duration::ZERO;

    for _ in 0..iterations {
        let start = Instant::now();
        db.replace_bib_chips(&chips).unwrap();
        let elapsed = start.elapsed();
        total_duration += elapsed;
    }

    let avg_duration = total_duration / iterations;
    println!(
        "BENCHMARK_RESULT: replace_bib_chips ({} items) avg time: {:?}",
        count, avg_duration
    );
    println!("BENCHMARK_MICROS: {}", avg_duration.as_micros());
}
