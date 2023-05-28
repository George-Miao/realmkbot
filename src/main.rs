#![feature(lazy_cell, type_changing_struct_update, duration_constants)]

#[macro_use]
extern crate log;

use std::{env, path::PathBuf, rc::Rc, sync::LazyLock};

use color_eyre::{eyre::Context, Result};
use redacted_debug::RedactedDebug;
use rust_tdlib::{
    client::{tdlib_client::TdJson, Client},
    types::*,
};
use serde::Deserialize;
use tap::Pipe;
use tokio::{select, signal::ctrl_c};

use crate::{
    db::{MessageRecord, Messages},
    tdlib::WorkerHandle,
};

mod db;
mod tdlib;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().unwrap();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "realmkbot=info");
    }
    pretty_env_logger::init();

    App::init().await?.load_chat_id().await?.run().await
}

struct App<ID> {
    config: &'static Config,
    db: Rc<Messages>,
    client: Client<TdJson>,
    chat_id: ID,
    handle: WorkerHandle,
}

impl App<()> {
    async fn init() -> Result<Self> {
        let config = Config::load();

        info!("Starting up...");
        info!("Using config: {:?}", config);

        let db = Messages::open(config.data_dir.join("main.db"))?.pipe(Rc::new);
        let (client, handle) = tdlib::init(config)
            .await
            .wrap_err("Failed to initialize TDLib")?;
        let this = Self {
            config,
            db,
            client,
            chat_id: (),
            handle,
        };
        this.populate().await?;
        Ok(this)
    }
}

impl App<i64> {
    async fn run(&mut self) -> Result<()> {
        info!("Running");

        loop {
            select! {
                update = self.handle.next_update() => {
                    if let Some(update) = update {
                        if let Err(e) = self.handle_update(update).await {
                            warn!("{e}")
                        }
                    } else {
                        break
                    }
                },
                _ = ctrl_c() => { break }
            };
        }

        info!("Shutting down");
        Ok(())
    }

    async fn handle_update(&self, update: Box<Update>) -> Result<()> {
        match *update {
            Update::NewInlineQuery(query) => {
                info!("New query from {}", query.sender_user_id());

                let results = if query.query().is_empty() {
                    self.db.random(10)?
                } else {
                    self.db.search(query.query(), 10)?
                }
                .into_iter()
                .map(InputInlineQueryResult::from)
                .collect();

                AnswerInlineQuery::builder()
                    .inline_query_id(query.id())
                    .cache_time(0)
                    .results(results)
                    .build()
                    .pipe(|a| self.client.answer_inline_query(a))
                    .await?;
            }
            Update::NewMessage(msg) => {
                if msg.message().chat_id() != self.chat_id {
                    return Result::<()>::Ok(());
                }

                info!("New message in channel");
                debug!("{msg:?}");

                let link = GetMessageLink::builder()
                    .chat_id(self.chat_id)
                    .message_id(msg.message().id())
                    .build()
                    .pipe(|r| self.client.get_message_link(r))
                    .await?;

                let Some(in_chat_id) = link.link().split('/').last().and_then(|x| x.parse().ok())
                else { return Ok(()); };

                let msg = MessageRecord::from_raw(msg.message().to_owned(), in_chat_id)?;
                self.db.insert_one(&msg)?;
            }
            u => {
                debug!("{u:?}")
            }
        }
        Ok(())
    }
}

impl<ID> App<ID> {
    async fn load_chat_id(self) -> Result<App<i64>> {
        let chat_id = GetMessageLinkInfo::builder()
            .url(format!(
                "tg:resolve?domain={}&post=1",
                self.config.chat_name
            ))
            .build()
            .pipe(|s| self.client.get_message_link_info(s))
            .await?
            .chat_id();

        Ok(App {
            chat_id,
            config: self.config,
            db: self.db,
            client: self.client,
            handle: self.handle,
        })
    }

    async fn populate(&self) -> Result<()> {
        info!("Populating");

        let mut consecutive_empty_msg = 0;
        let mut added = 0;

        for id in 1.. {
            if consecutive_empty_msg > 10 {
                break;
            }

            if self.db.exists(id)? {
                consecutive_empty_msg = 0;
                continue;
            }

            debug!("Getting {id}");
            let res = GetMessageLinkInfo::builder()
                .url(format!(
                    "tg:resolve?domain={}&post={}",
                    self.config.chat_name, id
                ))
                .build()
                .pipe(|s| self.client.get_message_link_info(s))
                .await?
                .message()
                .to_owned();

            let Some(msg) = res else {
                consecutive_empty_msg += 1;
                continue;
            };

            consecutive_empty_msg = 0;
            MessageRecord::from_raw(msg, id)?.pipe(|msg| self.db.insert_one(&msg))?;
            added += 1;
            debug!("Added");
        }

        info!("Done, {added} message(s) added");

        Ok(())
    }
}

#[derive(RedactedDebug, Deserialize)]
pub struct Config {
    #[redacted]
    pub bot_token: String,
    pub chat_name: String,
    #[redacted]
    pub api_id: i32,
    #[redacted]
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
            let config_dir = dirs::config_dir()
                .expect("Config dir cannot be found")
                .join("realmkbot");

            Figment::new()
                .merge(Env::raw())
                .merge(Toml::file("config.toml"))
                .merge(Toml::file(config_dir.join("config.toml")))
                .merge(Json::file("config.json"))
                .merge(Json::file(config_dir.join("config.json")))
                .extract()
                .expect("Failed to load config")
        });

        &CONFIG
    }

    pub fn tdlib_dir(&self) -> PathBuf {
        self.data_dir.join("tdlib")
    }
}
