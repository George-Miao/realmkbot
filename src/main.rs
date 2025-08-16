#![feature(type_changing_struct_update, duration_constants)]

#[macro_use]
extern crate log;

use std::{env, path::PathBuf, pin::pin, rc::Rc, sync::LazyLock};

use color_eyre::{
    Result,
    eyre::{Context, eyre},
};
use futures::{
    StreamExt,
    future::{Either, select},
    stream::FuturesUnordered,
};
use grammers_client::{
    Client, FixedReconnect, InitParams, Update,
    session::Session,
    types::{Chat, inline::query::Article},
};
use redacted_debug::RedactedDebug;
use serde::Deserialize;
use tap::Pipe;
use tokio::{select, signal::ctrl_c};

use crate::db::{MessageRecord, Messages};

mod db;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().unwrap();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "realmkbot=info");
    }
    pretty_env_logger::init();

    App::init()
        .await?
        .load_chat()
        .await?
        .populated()
        .await?
        .run()
        .await
}

struct App<C> {
    config: &'static Config,
    db: Rc<Messages>,
    client: Client,
    chat: C,
}

impl App<()> {
    async fn init() -> Result<Self> {
        let config = Config::load();

        info!("Starting up...");
        info!("Using config: {:?}", config);

        tokio::fs::create_dir_all(&config.data_dir).await?;

        let db = Messages::open(config.data_dir.join("main.db"))?.pipe(Rc::new);
        const RECON: FixedReconnect = FixedReconnect {
            attempts: 5,
            delay: std::time::Duration::from_secs(1),
        };
        let app_config = Config::load();

        let session = Session::load_file_or_create(config.data_dir.join("session"))
            .wrap_err("Failed to load session")?;

        let tg_config = grammers_client::Config {
            session,
            api_hash: app_config.api_hash.clone(),
            api_id: app_config.api_id,
            params: InitParams {
                device_model: "Desktop".to_owned(),
                system_version: "0.0".to_owned(),
                app_version: concat!("realmkbot ", env!("CARGO_PKG_VERSION")).to_owned(),
                system_lang_code: "en".to_owned(),
                lang_code: "en".to_owned(),
                catch_up: true,
                server_addr: None,
                flood_sleep_threshold: 0,
                update_queue_limit: None,
                reconnection_policy: &RECON,
            },
        };

        let client = grammers_client::Client::connect(tg_config).await.unwrap();
        client.bot_sign_in(&app_config.bot_token).await.unwrap();
        let me = client.get_me().await.unwrap().raw;
        info!("Logged in as: {:?}", me.username);

        let this = Self {
            config,
            db,
            client,
            chat: (),
        };

        Ok(this)
    }
}

impl App<Chat> {
    async fn run(&mut self) -> Result<()> {
        info!("Running");

        let mut pool = FuturesUnordered::new();

        loop {
            select! {
                update = self.client.next_update() => {
                    let update = update?;
                    pool.push(self.handle_update(update));
                },
                result = pool.next() => {
                    if let Some(Err(e)) = result {
                        warn!("Error handling update: {e:?}");
                    }
                }
                _ = ctrl_c() => {
                    info!("CTRL-C received, shutting down");
                    loop {
                        match select(pool.next(), pin!(ctrl_c())).await {
                            Either::Left(_) => {}
                            Either::Right(_) => {
                                info!("CTRL-C received again, shutting down immediately");
                                break;
                            }
                        }
                    }
                    break
                 }
            };
        }

        Ok(())
    }

    async fn populated(self) -> Result<Self> {
        self.populate().await?;
        Ok(self)
    }

    async fn populate(&self) -> Result<()> {
        const STEP: i32 = 128;

        if self.config.skip_populate {
            info!("Skipped populating");
            return Ok(());
        }

        info!("Populating");

        let mut consecutive_empty_msg = 0;
        let mut added = 0;

        'outter: for lower in (1..).step_by(STEP as _) {
            let upper = lower + STEP - 1;

            debug!("Getting {lower}..={upper}");

            let msg_ids = (lower..=upper).collect::<Vec<_>>();
            let res = self
                .client
                .get_messages_by_id(&self.chat, msg_ids.as_slice())
                .await?;

            for msg in res {
                if consecutive_empty_msg > 10 {
                    break 'outter;
                }

                let Some(msg) = msg else {
                    consecutive_empty_msg += 1;
                    continue;
                };

                if self.db.exists(msg.id())? {
                    consecutive_empty_msg = 0;
                    continue;
                }

                consecutive_empty_msg = 0;
                MessageRecord::from_raw(msg).pipe(|msg| self.db.insert_one(&msg))?;
                added += 1;
                debug!("Added");
            }

            info!("Added {added} message(s), wait for half minutes");
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }

        info!("Done, {added} message(s) added");

        Ok(())
    }

    async fn handle_update(&self, update: Update) -> Result<()> {
        match update {
            Update::MessageDeleted(update) => {
                if update.channel_id() != Some(self.chat.id()) {
                    debug!(
                        "Unknown channel, skip ({:?} != {})",
                        update.channel_id(),
                        self.chat.id()
                    );

                    return Result::<()>::Ok(());
                }

                info!("Message deleted in channel");
                debug!("{update:?}");

                self.db
                    .delete(update.messages())?
                    .pipe(|num| info!("{num} message(s) deleted"));
            }
            Update::InlineQuery(query) => {
                info!("New query from {}", query.sender().id());
                debug!("{query:?}");

                let results = if query.text().is_empty() {
                    self.db.random(10)?
                } else {
                    self.db.search(query.text(), 10)?
                }
                .into_iter()
                .map(Into::<Article>::into)
                .map(Into::into);

                query.answer(results).cache_time(0).send().await?;
            }
            Update::NewMessage(msg) => {
                if msg.chat().id() != self.chat.id() {
                    debug!(
                        "Unknown channel, skip ({} != {})",
                        msg.chat().id(),
                        self.chat.id()
                    );

                    return Result::<()>::Ok(());
                }

                info!("New message in channel");
                debug!("{msg:?}");

                let msg = MessageRecord::from_raw(msg);
                self.db.insert_one(&msg)?;
            }
            u => {
                debug!("{u:?}")
            }
        }
        Ok(())
    }
}

impl<C> App<C> {
    async fn load_chat(self) -> Result<App<Chat>> {
        let chat = self
            .client
            .resolve_username(&self.config.chat_name)
            .await?
            .ok_or_else(|| eyre!("Failed to resolve chat name {}", self.config.chat_name))?;

        Ok(App {
            chat,
            config: self.config,
            db: self.db,
            client: self.client,
        })
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

    #[serde(default)]
    pub skip_populate: bool,
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("data dir cannot be found")
        .join("realmkbot")
}

impl Config {
    pub fn load<'a>() -> &'a Self {
        use figment::{
            Figment,
            providers::{Env, Format, Json, Toml},
        };

        static CONFIG: LazyLock<Config> = LazyLock::new(|| {
            dotenvy::dotenv().ok();
            let config_dir = dirs::config_dir()
                .expect("Config dir cannot be found")
                .join("realmkbot");

            info!("Config dir: {}", config_dir.join("config.toml").display());

            Figment::new()
                .merge(Json::file(config_dir.join("config.json")))
                .merge(Toml::file(config_dir.join("config.toml")))
                .merge(Json::file("config.json"))
                .merge(Toml::file("config.toml"))
                .merge(Env::raw())
                .extract()
                .expect("Failed to load config")
        });

        &CONFIG
    }

    pub fn tdlib_dir(&self) -> PathBuf {
        self.data_dir.join("tdlib")
    }
}
