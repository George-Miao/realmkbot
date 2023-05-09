#![feature(lazy_cell)]

#[macro_use]
extern crate log;

use std::{env, path::PathBuf, sync::LazyLock};

use color_eyre::{eyre::Context, Result};
use rust_tdlib::types::*;
use serde::Deserialize;
use tap::Tap;

mod db;
mod tg;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    color_eyre::install().unwrap();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "realmkbot=info");
    }
    pretty_env_logger::init();

    run().await.unwrap()
}

async fn run() -> Result<()> {
    let config = Config::load();

    info!("Starting up...");
    info!("Using config: {:?}", config);

    let (client, handle) = tg::init(config, |_, u| {
        // info!("Update: {:?}", u);
        Ok(())
    })
    .await
    .wrap_err("Failed to initialize TDLib")?;

    let db = db::init(config).await?;

    for i in 100..110 {
        if let Some((msg, text)) = client
            .get_message_link_info(
                GetMessageLinkInfo::builder()
                    .url(format!("tg:resolve?domain={}&post={}", config.chat_name, i))
                    .build(),
            )
            .await?
            .message()
            .as_ref()
            .and_then(|msg| match msg.content() {
                MessageContent::MessageText(text) => Some((msg, text)),
                _ => None,
            })
        {
            let text = text.text().text();
            info!("{}: {}", i, text);
            db.insert(i, text)?;
        }
    }

    {
        let tx = db.tx(false)?;
        let mut cursor = tx.cursor()?;

        while let Some(Ok((index, msg))) = cursor.next() {
            info!("{}: {}", index, msg);
        }
    }
    handle.join().await?;

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub bot_token: String,
    pub chat_name: String,
    pub api_id: i32,
    pub api_hash: String,

    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("data dir cannot be found")
        .join("realmkbot")
}

impl Config {
    pub fn load<'a>() -> &'a Self {
        use figment::{
            providers::{Env, Format, Json, Toml},
            Figment,
        };

        static CONFIG: LazyLock<Config> = LazyLock::new(|| {
            dotenvy::dotenv().ok();

            Figment::new()
                .merge(Env::raw())
                .merge(Toml::file("config.toml"))
                .merge(Json::file("config.json"))
                .extract()
                .wrap_err("Failed to load config")
                .unwrap()
        });

        &CONFIG
    }
}
