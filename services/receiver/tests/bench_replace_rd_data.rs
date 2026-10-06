use receiver::Db;
use receiver::participants::Participant;
use std::time::Instant;

#[test]
fn bench_replace_rd_data() {
    let mut db = Db::open_in_memory().unwrap();

    let count = 10_000;
    let mut participants = Vec::with_capacity(count);
    let mut chips = Vec::with_capacity(count);
    for i in 0..count {
        participants.push(Participant {
            bib: i as i64,
            last: format!("LastName{i}"),
            first: format!("FirstName{i}"),
            affiliation: format!("Affiliation{i}"),
            gender: if i % 2 == 0 {
                "M".to_string()
            } else {
                "F".to_string()
            },
            division: Some((i % 20) as i32),
        });
        chips.push((i as i64, format!("CHIP{:08X}", i)));
    }

    let divisions: Vec<(i32, String)> = (0..50).map(|d| (d, format!("Division {d}"))).collect();

    // Warm up
    db.replace_rd_data(&participants, &chips, &divisions)
        .unwrap();

    let mut total_duration = std::time::Duration::ZERO;
    let iterations = 10;

    for _ in 0..iterations {
        let start = Instant::now();
        db.replace_rd_data(&participants, &chips, &divisions)
            .unwrap();
        total_duration += start.elapsed();
    }

    let avg = total_duration / iterations;
    println!("\n==================================================");
    println!("BENCHMARK: replace_rd_data for 10,000 items:");
    println!("Average time over {iterations} iterations: {:?}", avg);
    println!("==================================================\n");
}
