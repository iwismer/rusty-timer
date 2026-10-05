use rusqlite::{Connection, types::ToSql};
use timer_core::models::{ChipBib, Participant};
use timer_core::util::io::{read_bibchip_file, read_participant_file};

// Re-export types that main.rs needs
pub use timer_core::models::ReadType;
pub use timer_core::util::{is_empty_path, is_file, is_port, is_socket_addr};
pub use timer_core::workers::{ClientConnector, ClientPool, ReaderPool};

pub struct StreamerConfig {
    pub bib_chip_file_path: Option<String>,
    pub participants_file_path: Option<String>,
    pub readers: Vec<std::net::SocketAddrV4>,
    pub bind_port: u16,
    pub out_file: Option<String>,
    pub buffered_output: bool,
    pub read_type: ReadType,
}

pub fn create_tables(conn: &Connection) {
    conn.execute(
        "CREATE TABLE participant (
                  bib           INTEGER PRIMARY KEY,
                  first_name    TEXT NOT NULL,
                  last_name     TEXT NOT NULL,
                  gender        CHECK( gender IN ('M','F','X') ) NOT NULL DEFAULT 'X',
                  affiliation   TEXT,
                  division      INTEGER
                  )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE chip (
                  id     TEXT PRIMARY KEY,
                  bib    INTEGER NOT NULL
                  )",
        [],
    )
    .unwrap();
}

pub fn import_bib_chips(conn: &Connection, bib_chips: &[ChipBib]) {
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut stmt = tx
            .prepare_cached("INSERT OR IGNORE INTO chip (id, bib) VALUES (?1, ?2)")
            .unwrap();
        for c in bib_chips {
            stmt.execute([&c.id as &dyn ToSql, &c.bib]).unwrap();
        }
    }
    tx.commit().unwrap();
}

pub fn import_participants(conn: &Connection, participants: &[Participant]) {
    if participants.is_empty() {
        return;
    }

    let tx = conn.unchecked_transaction().unwrap();
    {
        const CHUNK_SIZE: usize = 50;
        const COLS_PER_ROW: usize = 6;

        let mut full_query = String::from(
            "INSERT OR IGNORE INTO participant (bib, first_name, last_name, gender, affiliation, division) VALUES ",
        );
        for i in 0..CHUNK_SIZE {
            if i > 0 {
                full_query.push_str(", ");
            }
            let p1 = i * COLS_PER_ROW + 1;
            let p2 = p1 + 1;
            let p3 = p1 + 2;
            let p4 = p1 + 3;
            let p5 = p1 + 4;
            let p6 = p1 + 5;
            use std::fmt::Write;
            write!(full_query, "(?{p1}, ?{p2}, ?{p3}, ?{p4}, ?{p5}, ?{p6})").unwrap();
        }

        let mut chunk_stmt = tx.prepare_cached(&full_query).unwrap();

        let mut single_stmt = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO participant (bib, first_name, last_name, gender, affiliation, division)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .unwrap();

        for chunk in participants.chunks(CHUNK_SIZE) {
            if chunk.len() == CHUNK_SIZE {
                let mut params: Vec<&dyn ToSql> = Vec::with_capacity(CHUNK_SIZE * COLS_PER_ROW);
                let mut gender_strs = Vec::with_capacity(CHUNK_SIZE);
                for p in chunk {
                    let g = match p.gender {
                        timer_core::models::Gender::M => "M",
                        timer_core::models::Gender::F => "F",
                        timer_core::models::Gender::X => "X",
                    };
                    gender_strs.push(g);
                }
                for (p, g) in chunk.iter().zip(gender_strs.iter()) {
                    params.push(&p.bib as &dyn ToSql);
                    params.push(&p.first_name as &dyn ToSql);
                    params.push(&p.last_name as &dyn ToSql);
                    params.push(g as &dyn ToSql);
                    params.push(&p.affiliation as &dyn ToSql);
                    params.push(&p.division as &dyn ToSql);
                }
                chunk_stmt.execute(params.as_slice()).unwrap();
            } else {
                for p in chunk {
                    let gender = match p.gender {
                        timer_core::models::Gender::M => "M",
                        timer_core::models::Gender::F => "F",
                        timer_core::models::Gender::X => "X",
                    };
                    single_stmt
                        .execute([
                            &p.bib as &dyn ToSql,
                            &p.first_name as &dyn ToSql,
                            &p.last_name as &dyn ToSql,
                            &gender as &dyn ToSql,
                            &p.affiliation as &dyn ToSql,
                            &p.division as &dyn ToSql,
                        ])
                        .unwrap();
                }
            }
        }
    }
    tx.commit().unwrap();
}

pub async fn run(config: StreamerConfig) {
    use futures::{future::FutureExt, future::select_all, pin_mut};
    use std::future::Future;
    use std::pin::Pin;
    use timer_core::models::Message;
    use timer_core::util::signal_handler;
    use tokio::sync::mpsc;

    let conn = Connection::open_in_memory().unwrap();
    create_tables(&conn);

    if let Some(ref path) = config.bib_chip_file_path {
        let bib_chips = read_bibchip_file(path).unwrap_or_default();
        import_bib_chips(&conn, &bib_chips);
    }
    if let Some(ref path) = config.participants_file_path {
        let participants = read_participant_file(path).unwrap_or_default();
        import_participants(&conn, &participants);
    }

    // Bus to send messages to client pool
    let (bus_tx, rx) = mpsc::channel::<Message>(1000);

    let client_pool = ClientPool::new(rx, Some(conn), config.out_file, config.buffered_output);
    let connector = ClientConnector::new(config.bind_port, bus_tx.clone()).await;
    let mut reader_pool = ReaderPool::new(config.readers, bus_tx.clone(), config.read_type);

    let fut_readers = reader_pool.begin().fuse();
    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();

    pin_mut!(fut_readers, fut_clients, fut_conn, fut_sig);
    let futures: Vec<Pin<&mut dyn Future<Output = ()>>> =
        vec![fut_readers, fut_clients, fut_conn, fut_sig];
    select_all(futures).await;
    // If any of them finish, end the program as something went wrong
    bus_tx.send(Message::SHUTDOWN).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use timer_core::models::{ChipBib, Gender, Participant};

    #[test]
    fn duplicate_chip_ids_are_ignored() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn);

        let chips = vec![
            ChipBib {
                id: "chip-1".to_owned(),
                bib: 101,
            },
            ChipBib {
                id: "chip-1".to_owned(),
                bib: 202,
            },
        ];

        import_bib_chips(&conn, &chips);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chip WHERE id = 'chip-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn duplicate_participant_bibs_are_ignored() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn);

        let participants = vec![
            Participant {
                chip_id: Vec::new(),
                bib: 77,
                first_name: "Jane".to_owned(),
                last_name: "Doe".to_owned(),
                gender: Gender::F,
                age: None,
                affiliation: None,
                division: None,
            },
            Participant {
                chip_id: Vec::new(),
                bib: 77,
                first_name: "Janet".to_owned(),
                last_name: "Roe".to_owned(),
                gender: Gender::F,
                age: None,
                affiliation: None,
                division: None,
            },
        ];

        import_participants(&conn, &participants);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM participant WHERE bib = 77",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    #[ignore]
    fn bench_import_bib_chips() {
        use std::time::Instant;

        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn);

        let chips: Vec<ChipBib> = (0..10_000)
            .map(|i| ChipBib {
                id: format!("chip-{i}"),
                bib: i,
            })
            .collect();

        let start = Instant::now();
        import_bib_chips(&conn, &chips);
        let elapsed = start.elapsed();

        println!("import_bib_chips (10,000 items): {:?}", elapsed);
    }

    #[test]
    #[ignore]
    fn bench_import_participants() {
        use std::time::Instant;

        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn);

        let participants: Vec<Participant> = (0..10_000)
            .map(|i| Participant {
                chip_id: Vec::new(),
                bib: i,
                first_name: "Jane".to_owned(),
                last_name: "Doe".to_owned(),
                gender: Gender::F,
                age: Some(30),
                affiliation: Some("Club".to_owned()),
                division: Some(1),
            })
            .collect();

        let start = Instant::now();
        import_participants(&conn, &participants);
        let elapsed = start.elapsed();

        println!("import_participants (10,000 items): {:?}", elapsed);
    }
}
