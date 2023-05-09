use color_eyre::{eyre::Context, Result};
use jammdb::{Bucket, Cursor, Tx, DB};
use rust_tdlib::types::Message;
use serde::{Deserialize, Serialize};
use tap::Pipe;

use crate::Config;

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageRecord {
    index: u32,
    #[serde(flatten)]
    message: Message,
}

pub struct Messages {
    db: DB,
}

pub struct MessageTx<'tx>(Tx<'tx>);

impl<'tx> MessageTx<'tx> {
    const LAST_INDEX_KEY: &'static str = "last_index";

    fn insert(self, index: u32, message: &'tx str) -> Result<()> {
        let meta = self.meta()?;

        let last_index = if let Some(data) = meta.get_kv(Self::LAST_INDEX_KEY) {
            u32::from_le_bytes(data.value().try_into().wrap_err("Bad last index")?)
        } else {
            0
        };

        if last_index < index {
            let bytes = index.to_le_bytes();
            self.message()?.put(bytes, message)?;

            meta.put(Self::LAST_INDEX_KEY, bytes)?;
        }

        self.commit()?;

        Ok(())
    }

    fn commit(self) -> Result<()> {
        self.0.commit().wrap_err("Failed to commit transaction")
    }

    fn meta<'b>(&'b self) -> Result<Bucket<'b, 'tx>> {
        self.0
            .get_or_create_bucket("meta")
            .wrap_err("Failed to open or create bucket `meta`")
    }

    fn message<'b>(&'b self) -> Result<Bucket<'b, 'tx>> {
        self.0
            .get_or_create_bucket("message")
            .wrap_err("Failed to open or create bucket `message`")
    }

    pub fn cursor<'b>(&'b self) -> Result<MessageCursor<'b, 'tx>> {
        self.message()?.cursor().pipe(MessageCursor).pipe(Ok)
    }
}

pub struct MessageCursor<'b, 'tx>(Cursor<'b, 'tx>);

impl<'b, 'tx> Iterator for MessageCursor<'b, 'tx> {
    type Item = Result<(u32, String)>;

    fn next(&mut self) -> Option<Self::Item> {
        let data = self.0.next()?;
        let (key, value) = data.kv().kv();
        (|| {
            Ok((
                u32::from_le_bytes(key.try_into().wrap_err("Bad index")?),
                String::from_utf8(value.to_vec()).wrap_err("Bad content")?,
            ))
        })()
        .pipe(Some)
    }
}

impl Messages {
    pub fn tx(&self, writable: bool) -> Result<MessageTx> {
        self.db
            .tx(writable)
            .wrap_err("Failed to start transaction")
            .map(MessageTx)
    }

    pub fn insert(&self, index: u32, message: &str) -> Result<()> {
        self.tx(true)?.insert(index, message)
    }
}

pub async fn init(config: &Config) -> Result<Messages> {
    let db = DB::open(config.data_dir.join("realmkbot.db").as_path())
        .wrap_err("Failed to open database")?;

    Ok(Messages { db })
}

#[cfg(test)]
mod test {
    use color_eyre::Result;
    use jammdb::DB;

    #[test]
    fn test_db() -> Result<()> {
        let db = DB::open("data/123.db")?;

        let tx = db.tx(true)?;
        tx.create_bucket("test")?;
        tx.commit()?;

        Ok(())
    }
}
