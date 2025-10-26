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
use rusqlite::{Connection, params};
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};
use tap::Pipe;

#[derive(Debug)]
pub struct Messages(Connection);

impl Messages {
    #[inline]
    pub fn open(p: impl AsRef<Path>) -> Result<Self> {
        Connection::open(p)?.pipe(Self).pre_start()?.pipe(Ok)
    }

    #[inline]
    fn pre_start(mut self) -> Result<Self> {
        let migrations = Migrations::new(vec![M::up(
            "CREATE TABLE message  (
                id           INTEGER PRIMARY KEY,
                text         TEXT,
                is_forwarded BOOLEAN,
                raw          BLOB
            )",
        )]);

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
             is_forwarded = TRUE LIMIT ?2",
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
            .query_map([], |row| row.get::<_, i32>(0))?
            .collect::<rusqlite::Result<BTreeSet<i32>>>()
            .wrap_err("Failed to collect existing ids")
    }
}

impl Deref for Messages {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Messages {
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

impl From<SearchResult> for Article {
    fn from(x: SearchResult) -> Self {
        let msg = InputMessage::text(&x.text);
        Article::new(x.text, msg)
            .id(format!("{}", x.id))
            .description(format!("#{}", x.id))
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
