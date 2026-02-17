use receiver::Db;

fn main() {
    tracing_subscriber::fmt::init();
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rusty-timer")
        .join("receiver");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = data_dir.join("receiver.sqlite3");
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("FATAL: failed to open DB: {e}");
        std::process::exit(1);
    });
    db.integrity_check().unwrap_or_else(|e| {
        eprintln!("FATAL: integrity_check failed: {e}");
        std::process::exit(1);
    });
    tracing::info!("receiver started");
}
