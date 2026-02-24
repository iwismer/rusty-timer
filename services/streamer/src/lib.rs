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
    for c in bib_chips {
        conn.execute(
            "INSERT OR IGNORE INTO chip (id, bib)
                    VALUES (?1, ?2)",
            [&c.id as &dyn ToSql, &c.bib],
        )
        .unwrap();
    }
}

pub fn import_participants(conn: &Connection, participants: &[Participant]) {
    for p in participants {
        let gender = format!("{}", p.gender);
        conn.execute(
            "INSERT OR IGNORE INTO participant (bib, first_name, last_name, gender, affiliation, division)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            [
                &p.bib as &dyn ToSql,
                &p.first_name as &dyn ToSql,
                &p.last_name as &dyn ToSql,
                &gender as &dyn ToSql,
                &p.affiliation as &dyn ToSql,
                &p.division as &dyn ToSql,
            ],
        )
        .unwrap();
    }
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
}
