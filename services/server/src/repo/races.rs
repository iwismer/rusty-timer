use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

pub struct RaceRow {
    pub race_id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub participant_count: i64,
    pub chip_count: i64,
}

pub struct ParticipantRow {
    pub bib: i32,
    pub first_name: String,
    pub last_name: String,
    pub gender: String,
    pub affiliation: Option<String>,
    pub chip_ids: Vec<String>,
}

pub struct UnmatchedChipRow {
    pub chip_id: String,
    pub bib: i32,
}

pub async fn list_races(pool: &PgPool) -> Result<Vec<RaceRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT r.race_id, r.name, r.created_at,
                  (SELECT COUNT(*) FROM participants p WHERE p.race_id = r.race_id) AS participant_count,
                  (SELECT COUNT(*) FROM chips c WHERE c.race_id = r.race_id) AS chip_count
           FROM races r ORDER BY r.created_at DESC"#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RaceRow {
            race_id: r.get("race_id"),
            name: r.get("name"),
            created_at: r.get("created_at"),
            participant_count: r.get("participant_count"),
            chip_count: r.get("chip_count"),
        })
        .collect())
}

pub async fn create_race(pool: &PgPool, name: &str) -> Result<RaceRow, sqlx::Error> {
    let row =
        sqlx::query("INSERT INTO races (name) VALUES ($1) RETURNING race_id, name, created_at")
            .bind(name)
            .fetch_one(pool)
            .await?;

    Ok(RaceRow {
        race_id: row.get("race_id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        participant_count: 0,
        chip_count: 0,
    })
}

pub async fn delete_race(pool: &PgPool, race_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM races WHERE race_id = $1")
        .bind(race_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn race_exists(pool: &PgPool, race_id: Uuid) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT EXISTS(SELECT 1 FROM races WHERE race_id = $1) AS exists")
        .bind(race_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<bool, _>("exists"))
}

pub async fn replace_participants(
    pool: &PgPool,
    race_id: Uuid,
    participants: &[(i32, &str, &str, &str, Option<&str>)],
) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM participants WHERE race_id = $1")
        .bind(race_id)
        .execute(&mut *tx)
        .await?;
    let mut count = 0u64;
    for (bib, first_name, last_name, gender, affiliation) in participants {
        sqlx::query(
            "INSERT INTO participants (race_id, bib, first_name, last_name, gender, affiliation) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(race_id)
        .bind(bib)
        .bind(first_name)
        .bind(last_name)
        .bind(gender)
        .bind(*affiliation)
        .execute(&mut *tx)
        .await?;
        count += 1;
    }
    tx.commit().await?;
    Ok(count)
}

pub async fn replace_chips(
    pool: &PgPool,
    race_id: Uuid,
    chips: &[(&str, i32)],
) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM chips WHERE race_id = $1")
        .bind(race_id)
        .execute(&mut *tx)
        .await?;
    let mut count = 0u64;
    for (chip_id, bib) in chips {
        sqlx::query(
            "INSERT INTO chips (race_id, chip_id, bib) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(race_id)
        .bind(chip_id)
        .bind(bib)
        .execute(&mut *tx)
        .await?;
        count += 1;
    }
    tx.commit().await?;
    Ok(count)
}

pub async fn list_participants(
    pool: &PgPool,
    race_id: Uuid,
) -> Result<Vec<ParticipantRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT p.bib, p.first_name, p.last_name, p.gender, p.affiliation,
                  COALESCE(array_agg(c.chip_id) FILTER (WHERE c.chip_id IS NOT NULL), '{}') AS chip_ids
           FROM participants p
           LEFT JOIN chips c ON c.race_id = p.race_id AND c.bib = p.bib
           WHERE p.race_id = $1
           GROUP BY p.bib, p.first_name, p.last_name, p.gender, p.affiliation
           ORDER BY p.bib"#,
    )
    .bind(race_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ParticipantRow {
            bib: r.get("bib"),
            first_name: r.get("first_name"),
            last_name: r.get("last_name"),
            gender: r.get("gender"),
            affiliation: r.get("affiliation"),
            chip_ids: r.get("chip_ids"),
        })
        .collect())
}

pub async fn list_unmatched_chips(
    pool: &PgPool,
    race_id: Uuid,
) -> Result<Vec<UnmatchedChipRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT c.chip_id, c.bib
           FROM chips c
           LEFT JOIN participants p ON p.race_id = c.race_id AND p.bib = c.bib
           WHERE c.race_id = $1 AND p.bib IS NULL
           ORDER BY c.bib"#,
    )
    .bind(race_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UnmatchedChipRow {
            chip_id: r.get("chip_id"),
            bib: r.get("bib"),
        })
        .collect())
}
