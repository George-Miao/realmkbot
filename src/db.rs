use std::{
    ops::{Deref, DerefMut},
    path::Path,
};

use color_eyre::{eyre::Context, Result};
use rusqlite::{params, Connection};
use rusqlite_migration::{Migrations, M};
use rust_tdlib::types::{
    FormattedText, InputInlineQueryResult, InputInlineQueryResultArticle, InputMessageContent,
    InputMessageText, Message, MessageContent,
};
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
        let migrations = Migrations::new(vec![
            M::up(
                "CREATE TABLE message  (
                id           INTEGER PRIMARY KEY,
                in_chat_id   INTEGER NOT NULL,
                text         TEXT,
                is_forwarded BOOLEAN,
                raw          BLOB
            )",
            ),
            M::up("CREATE UNIQUE INDEX message_in_chat_id ON message (in_chat_id)"),
        ]);

        self.pragma_update(None, "journal_mode", "WAL")?;
        migrations.to_latest(&mut self)?;

        Ok(self)
    }

    // pub fn max_in_chat_id(&self) -> Result<i64> {
    //     self.query_row("SELECT max(in_chat_id) FROM message", (), |res|
    // res.get(0))         .wrap_err("Failed to get `max_in_chat_id`")
    // }

    pub fn random(&self, limit: u8) -> Result<Vec<SearchResult>> {
        self.prepare(
            "SELECT in_chat_id, text FROM message WHERE is_forwarded = TRUE AND text IS NOT NULL \
             ORDER BY RANDOM() LIMIT ?",
        )?
        .query_map([limit], |row| {
            SearchResult {
                in_chat_id: row.get(0)?,
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
            "SELECT in_chat_id, text FROM message WHERE text IS NOT NULL AND text LIKE ?1 AND \
             is_forwarded = TRUE LIMIT ?2",
        )?
        .query_map(params![format!("%{reg}%"), limit], |row| {
            SearchResult {
                in_chat_id: row.get(0)?,
                text: row.get(1)?,
            }
            .pipe(Ok)
        })
        .wrap_err("Failed to search")?
        .collect::<rusqlite::Result<Vec<SearchResult>>>()
        .wrap_err("Failed to collect search result")
    }

    pub fn insert_one(&self, msg: &MessageRecord) -> Result<()> {
        self.execute(
            "INSERT OR REPLACE INTO message (id, in_chat_id, text, is_forwarded, raw) VALUES (?1, \
             ?2, ?3, ?4, ?5)",
            (
                &msg.id,
                &msg.in_chat_id,
                &msg.text,
                &msg.is_forwarded,
                &msg.raw,
            ),
        )
        .wrap_err("Failed to insert message")
        .map(|_| ())
    }

    pub fn exists(&self, in_chat_id: i64) -> Result<bool> {
        self.query_row(
            "SELECT EXISTS(SELECT 1 FROM message WHERE in_chat_id = ?1)",
            [in_chat_id],
            |res| res.get(0),
        )
        .wrap_err("Failed to check if message exists")
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
    pub id: i64,
    pub in_chat_id: i64,
    pub text: Option<String>,
    pub is_forwarded: bool,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub in_chat_id: i64,
    pub text: String,
}

impl MessageRecord {
    // pub fn get_raw(&self) -> Result<Message, serde_json::Error> {
    //     serde_json::from_slice(&self.raw)
    // }

    pub fn from_raw(msg: Message, in_chat_id: i64) -> Result<Self, serde_json::Error> {
        let text = match msg.content() {
            MessageContent::MessageText(text) => text.text().text().to_owned().pipe(Some),
            _ => None,
        };

        Self {
            id: msg.id(),
            in_chat_id,
            text,
            is_forwarded: msg.forward_info().is_some(),
            raw: serde_json::to_vec(&msg)?,
        }
        .pipe(Ok)
    }
}

impl From<SearchResult> for InputInlineQueryResult {
    fn from(value: SearchResult) -> Self {
        InputInlineQueryResultArticle::builder()
            .id(value.in_chat_id.to_string())
            .description(format!("#{}", value.in_chat_id))
            .title(value.text.clone())
            .hide_url(true)
            .input_message_content(
                FormattedText::builder()
                    .text(value.text)
                    .build()
                    .pipe(|text| InputMessageText::builder().text(text).build())
                    .pipe(InputMessageContent::InputMessageText),
            )
            .build()
            .pipe(InputInlineQueryResult::Article)
    }
}
