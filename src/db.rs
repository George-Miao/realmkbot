use std::{
    collections::BTreeSet,
    ops::{Deref, DerefMut},
    path::Path,
};

use color_eyre::{Result, eyre::Context};
use grammers_client::{
    InputMessage,
    grammers_tl_types::Serializable,
    types::{Message, inline::query::Article},
};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};
use tap::Pipe;

pub const USER_STATS_ID: &str = "stats";

#[derive(Debug)]
pub struct Database(Connection);

impl Database {
    #[inline]
    pub fn open(p: impl AsRef<Path>) -> Result<Self> {
        Connection::open(p)?.pipe(Self).pre_start()?.pipe(Ok)
    }

    #[inline]
    fn pre_start(mut self) -> Result<Self> {
        let migrations = Migrations::new(vec![
            M::up(
                "CREATE TABLE message (
                    id           INTEGER PRIMARY KEY,
                    text         TEXT,
                    is_forwarded BOOLEAN,
                    raw          BLOB
                )",
            ),
            M::up(
                "CREATE TABLE user (
                    user_id      INTEGER PRIMARY KEY,
                    count        INTEGER
                )",
            ),
        ]);

        self.pragma_update(None, "journal_mode", "WAL")?;
        migrations.to_latest(&mut self)?;

        Ok(self)
    }

    pub fn random(&self, limit: u8) -> Result<Vec<SearchResult>> {
        self.prepare(
            "SELECT id, text FROM message WHERE is_forwarded = TRUE AND text IS NOT NULL ORDER BY \
             RANDOM() LIMIT ?",
        )?
        .query_map([limit], |row| {
            SearchResult {
                id: row.get(0)?,
                text: row.get(1)?,
            }
            .pipe(Ok)
        })
        .wrap_err("Failed to random")?
        .collect::<rusqlite::Result<Vec<SearchResult>>>()
        .wrap_err("Failed to collect search result")
    }

    pub fn search(&self, reg: &str, limit: u8) -> Result<Vec<SearchResult>> {
        self.prepare(
            "SELECT id, text FROM message WHERE text IS NOT NULL AND text LIKE ?1 AND \
             is_forwarded = TRUE ORDER BY RANDOM() LIMIT ?2",
        )?
        .query_map(params![format!("%{reg}%"), limit], |row| {
            SearchResult {
                id: row.get(0)?,
                text: row.get(1)?,
            }
            .pipe(Ok)
        })
        .wrap_err("Failed to search")?
        .collect::<rusqlite::Result<Vec<SearchResult>>>()
        .wrap_err("Failed to collect search result")
    }

    pub fn upsert_one(&self, msg: &MessageRecord) -> Result<()> {
        self.execute(
            r"INSERT OR REPLACE INTO message (id, text, is_forwarded, raw) VALUES (?1, ?2, ?3, ?4)",
            (&msg.id, &msg.text, &msg.is_forwarded, &msg.raw),
        )
        .wrap_err("Failed to insert message")
        .map(|_| ())
    }

    pub fn delete(&self, ids: &[i32]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        info!("Deleting {ids:?}");

        let mut num = 0;
        for id in ids {
            num += self.execute("DELETE FROM message WHERE id = ?1", (id,))?;
        }

        Ok(num)
    }

    pub fn existing_ids(&self) -> Result<BTreeSet<i32>> {
        self.prepare("SELECT id FROM message")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<BTreeSet<i32>>>()
            .wrap_err("Failed to collect existing ids")
    }

    pub fn bump_user_count(&self, user_id: i64) -> Result<()> {
        self.execute(
            r"INSERT INTO user (user_id, count) VALUES (?1, 1)
              ON CONFLICT(user_id) DO UPDATE SET count = count + 1",
            (user_id,),
        )
        .wrap_err("Failed to bump user count")
        .map(|_| ())
    }

    pub fn get_user_stats(&self, user_id: i64) -> Result<Option<UserStat>> {
        self.prepare(
            "SELECT count, count(*), COUNT(count <= u.count) FROM user u HAVINg user_id = ?1",
        )?
        .query_row((user_id,), |row| {
            Ok(UserStat {
                user_id,
                count: row.get(0)?,
                total_users: row.get(1)?,
                lower_users: row.get(2)?,
            })
        })
        .optional()
        .wrap_err("Failed to get user stats")
    }
}

impl Deref for Database {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Database {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: i32,
    pub text: Option<String>,
    pub is_forwarded: bool,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: i32,
    pub text: String,
}

pub struct UserStat {
    pub user_id: i64,
    pub count: u32,
    pub total_users: u32,
    pub lower_users: u32,
}

impl From<SearchResult> for Article {
    fn from(x: SearchResult) -> Self {
        let msg = InputMessage::text(&x.text);
        Article::new(x.text, msg)
            .id(format!("{}", x.id))
            .description(format!("#{}", x.id))
    }
}

impl From<UserStat> for Article {
    fn from(x: UserStat) -> Self {
        let msg = format!(
            "我发了{}次 mk 语录，在模仿 mk 大赛中获得了第{}名的好成绩！",
            x.count,
            x.total_users - x.lower_users + 1,
        );
        let desc = format!(
            "击败了 {}% 的群友",
            (x.lower_users as f64 / x.total_users as f64) * 100.0
        );

        Article::new(msg.clone(), InputMessage::text(msg))
            .description(desc)
            .id(USER_STATS_ID)
    }
}

impl MessageRecord {
    pub fn from_raw(msg: Message) -> Self {
        let text = match msg.text() {
            "" => None,
            text => Some(text.to_string()),
        };

        Self {
            id: msg.id(),
            text,
            is_forwarded: msg.forward_header().is_some(),
            raw: msg.raw.to_bytes(),
        }
    }
}
