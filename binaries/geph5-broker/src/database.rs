use std::{ops::Deref, str::FromStr, sync::LazyLock, time::Duration};

use async_io::Timer;
use geph5_broker_protocol::{BridgeDescriptor, Credential};
use moka::future::Cache;

use rand::Rng;
use sqlx::{
    pool::PoolOptions,
    postgres::{PgConnectOptions, PgSslMode},
    prelude::*,
    types::chrono::Utc,
    PgPool,
};

use crate::CONFIG_FILE;

pub static POSTGRES: LazyLock<PgPool> = LazyLock::new(|| {
    smolscale::block_on(
        PoolOptions::new()
            .max_connections(32)
            .acquire_timeout(Duration::from_secs(60))
            .max_lifetime(Duration::from_secs(600))
            .connect_with({
                let cfg = CONFIG_FILE.wait();
                let mut opts = PgConnectOptions::from_str(&cfg.postgres_url).unwrap();
                if let Some(postgres_root_cert) = &cfg.postgres_root_cert {
                    opts = opts
                        .ssl_mode(PgSslMode::VerifyFull)
                        .ssl_root_cert(postgres_root_cert);
                }
                opts
            }),
    )
    .unwrap()
});

/// This loop is used for garbage-collecting stale data from the database.
#[tracing::instrument]
pub async fn database_gc_loop() -> anyhow::Result<()> {
    tracing::info!("starting the database GC loop");
    loop {
        let sleep_time = Duration::from_secs_f64(rand::thread_rng().gen_range(60.0..120.0));
        tracing::debug!("sleeping {:?}", sleep_time);
        Timer::after(sleep_time).await;
        let res = sqlx::query("delete from exits_new where expiry < extract(epoch from now())")
            .execute(POSTGRES.deref())
            .await?;
        tracing::debug!(rows_affected = res.rows_affected(), "cleaned up exits");
        let res = sqlx::query("delete from bridges_new where expiry < extract(epoch from now())")
            .execute(POSTGRES.deref())
            .await?;
        tracing::debug!(rows_affected = res.rows_affected(), "cleaned up bridges");
    }
}

#[derive(FromRow)]
pub struct ExitRow {
    pub pubkey: [u8; 32],
    pub c2e_listen: String,
    pub b2e_listen: String,
    pub country: String,
    pub city: String,
    pub load: f32,
    pub expiry: i64,
}

pub async fn insert_exit(exit: &ExitRow) -> anyhow::Result<()> {
    sqlx::query(
        r"INSERT INTO exits_new (pubkey, c2e_listen, b2e_listen, country, city, load, expiry)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (pubkey) DO UPDATE 
        SET c2e_listen = EXCLUDED.c2e_listen, 
            b2e_listen = EXCLUDED.b2e_listen, 
            country = EXCLUDED.country, 
            city = EXCLUDED.city, 
            load = EXCLUDED.load, 
            expiry = EXCLUDED.expiry
        ",
    )
    .bind(exit.pubkey)
    .bind(&exit.c2e_listen)
    .bind(&exit.b2e_listen)
    .bind(&exit.country)
    .bind(&exit.city)
    .bind(exit.load)
    .bind(exit.expiry)
    .execute(POSTGRES.deref())
    .await?;
    Ok(())
}

pub async fn query_bridges(key: &str) -> anyhow::Result<Vec<BridgeDescriptor>> {
    static CACHE: LazyLock<Cache<String, Vec<BridgeDescriptor>>> = LazyLock::new(|| {
        Cache::builder()
            .time_to_live(Duration::from_secs(60))
            .build()
    });

    CACHE.try_get_with(key.to_string(), async {
        let raw: Vec<(String, String, String, i64)> = sqlx::query_as(r"
        select distinct on (pool) listen, cookie, pool, expiry from bridges_new order by pool, encode(digest(listen || $1, 'sha256'), 'hex');
        ").bind(key).fetch_all(POSTGRES.deref()).await?;
        anyhow::Ok(raw
        .into_iter()
        .map(|row| BridgeDescriptor {
            control_listen: row.0.parse().unwrap(),
            control_cookie: row.1,
            pool: row.2,
            expiry: row.3 as _,
        })
        .collect())
    }).await.map_err(|e| anyhow::anyhow!(e))
}

pub async fn new_pow_nonce() -> anyhow::Result<String> {
    let nonce = hex::encode(rand::thread_rng().gen::<[u8; 16]>());
    sqlx::query("INSERT INTO pow_nonces (nonce) VALUES ($1) ON CONFLICT (nonce) DO NOTHING")
        .execute(&*POSTGRES)
        .await?;
    Ok(nonce)
}

pub async fn consume_pow_nonce(nonce: String) -> anyhow::Result<()> {
    // Try to delete the nonce from the database. If no rows are affected, the nonce does not exist.
    let res = sqlx::query("DELETE FROM pow_nonces WHERE nonce = $1")
        .bind(nonce)
        .execute(POSTGRES.deref())
        .await?;

    // If no rows were affected, the nonce was not found
    if res.rows_affected() == 0 {
        return Err(anyhow::anyhow!("nonce not found or already consumed"));
    }

    Ok(())
}

pub async fn new_user() -> anyhow::Result<Credential> {
    let mut txn = POSTGRES.begin().await?;
    let row = sqlx::query("insert into users (createtime) values ($1)")
        .bind(Utc::now().naive_utc())
        .fetch_one(&mut *txn)
        .await?;
    let user_id: i32 = row.get(0);

    // Bearer tokens are 96-bit numbers encoded in base32
    let token = base32::encode(base32::Alphabet::Z, &rand::thread_rng().gen::<[u8; 12]>());
    let token_hash = blake3::hash(token.as_bytes()).to_string();
    // we store the blake3 in the database
    sqlx::query("insert into auth_tokens (id, bearer_hash) values ($1, $2)")
        .bind(user_id)
        .bind(token_hash)
        .execute(&mut *txn)
        .await?;

    txn.commit().await?;
    Ok(Credential::Bearer(token))
}
