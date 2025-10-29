#![feature(type_changing_struct_update, duration_constants)]

#[macro_use]
extern crate log;

use std::{collections::BTreeSet, env, path::PathBuf, sync::Arc};

use color_eyre::{
    Result,
    eyre::{Context, ContextCompat, eyre},
};
use futures::TryFutureExt;
use grammers_client::{
    Client, ClientConfiguration, Update, UpdatesConfiguration,
    client::bots::AuthorizationError,
    grammers_tl_types as tl,
    session::{UpdatesLike, storages::TlSession},
    types::{Peer, update::Article},
};
use grammers_mtsender::{ConnectionParams, SenderPool};
use redacted_debug::RedactedDebug;
use serde::Deserialize;
use tap::Pipe;
use tokio::{spawn, sync::mpsc::UnboundedReceiver, task::JoinSet};

use crate::{
    db::{Database, MessageRecord, USER_STATS_ID},
    util::SkippingIter,
};

mod db;
mod util;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().unwrap();
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "realmkbot=info,grammers_client=debug");
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
    config: Config,
    db: Database,
    client: Client,
    updates: Option<UnboundedReceiver<UpdatesLike>>,
    chat: C,
}

impl App<()> {
    async fn init() -> Result<Self> {
        let config = Config::load()?;

        info!("Starting up...");
        info!("Using config: {:?}", config);

        tokio::fs::create_dir_all(&config.data_dir).await?;

        let db = Database::open(config.data_dir.join("main.db"))?;

        let session = TlSession::load_file_or_create(config.data_dir.join("session"))
            .wrap_err("Failed to load session")?
            .pipe(Arc::new);

        let mut param = ConnectionParams::default();
        param.device_model = "Desktop".to_owned();
        param.system_version = "0.0".to_owned();
        param.app_version = concat!("realmkbot ", env!("CARGO_PKG_VERSION")).to_owned();
        param.system_lang_code = "en".to_owned();
        param.lang_code = "en".to_owned();

        let pool = SenderPool::with_configuration(session, config.api_id, param);
        let client = grammers_client::Client::with_configuration(
            &pool,
            ClientConfiguration {
                flood_sleep_threshold: 0,
            },
        );

        spawn(pool.runner.run());

        let me = client
            .bot_sign_in(&config.bot_token, &config.api_hash)
            .map_err(|e| match e {
                AuthorizationError::Gen(e) => panic!("Authorization error: {:?}", e),
                AuthorizationError::Invoke(e) => e,
            })
            .await
            .expect("Failed to sign in bot");

        info!("Logged in as: {:?}", me.username());

        let this = Self {
            config,
            updates: Some(pool.updates),
            db,
            client,
            chat: (),
        };

        Ok(this)
    }
}

impl App<Peer> {
    async fn run(&mut self) -> Result<()> {
        let updates = self.updates.take().expect("Cannot run the client twice");

        let config = UpdatesConfiguration {
            catch_up: true,
            update_queue_limit: Some(128),
        };

        let mut stream = self.client.stream_updates(updates, config);

        loop {
            let update = stream.next().await?;
            self.handle_update(update).await?;
        }
    }

    async fn handle_update(&self, update: Update) -> Result<()> {
        match update {
            Update::InlineQuery(query) => {
                info!("New query from {}", query.sender().bare_id());
                debug!("{query:?}");

                let results = if query.text().is_empty() {
                    self.db.random(10)?
                } else {
                    self.db.search(query.text(), 10)?
                }
                .into_iter()
                .map(Into::<Article>::into);

                let user_stat = self
                    .db
                    .get_user_stats(query.sender().bare_id())?
                    .into_iter()
                    .map(Into::<Article>::into);

                query
                    .answer(user_stat.chain(results))
                    .cache_time(0)
                    .send()
                    .await?;
            }
            Update::InlineSend(send) => {
                let id = send.sender().bare_id();
                if send.result_id() == USER_STATS_ID {
                    info!("{id} requested stats");
                    return Ok(());
                }
                info!("Message sent by {id}");
                self.db.bump_user_count(id)?;
            }
            Update::NewMessage(msg) => {
                if msg.chat_id() != self.chat.id() {
                    debug!(
                        "Unknown channel, skip ({} != {})",
                        msg.chat_id(),
                        self.chat.id()
                    );

                    return Result::<()>::Ok(());
                }

                info!("New message in channel");
                debug!("{msg:?}");

                let msg = MessageRecord::from_raw(&msg);
                self.db.upsert_one(&msg)?;
            }
            Update::MessageDeleted(update) => {
                if update.channel_id() != Some(self.chat.id().bare_id()) {
                    debug!(
                        "Unknown chat, skip ({:?} != {})",
                        update.channel_id(),
                        self.chat.id().bare_id()
                    );

                    return Result::<()>::Ok(());
                }

                info!("Message deleted in channel");
                debug!("{update:?}");

                self.db
                    .delete(update.messages())?
                    .pipe(|num| info!("{num} message(s) deleted"));
            }
            u => {
                debug!("{u:?}")
            }
        }

        Ok(())
    }

    async fn populated(self) -> Result<Self> {
        self.populate().await?;
        Ok(self)
    }

    async fn populate(&self) -> Result<()> {
        if self.config.skip_populate {
            info!("Skipped populating");
            return Ok(());
        }

        info!("Populating");

        let mut consecutive_empty_msg = 0;
        let mut added = 0;

        let existing_ids = if self.config.force_repopulate {
            BTreeSet::new()
        } else {
            self.db.existing_ids()?
        };

        let mut iter = SkippingIter::new(&existing_ids);

        'outter: loop {
            let msg_ids = (&mut iter).take(100).collect::<Vec<_>>();

            let res = self
                .client
                .get_messages_by_id(&self.chat, msg_ids.as_slice())
                .await?;

            for msg in res {
                // Assume there're no more messages after 10 consecutive empty messages
                if consecutive_empty_msg > 10 {
                    break 'outter;
                }

                let Some(msg) = msg else {
                    consecutive_empty_msg += 1;
                    continue;
                };
                if matches!(msg.raw, tl::enums::Message::Empty(_)) {
                    consecutive_empty_msg += 1;
                    continue;
                }

                let id = msg.id();

                consecutive_empty_msg = 0;

                MessageRecord::from_raw(&msg).pipe(|msg| self.db.upsert_one(&msg))?;

                added += 1;
                debug!("Added #{id}");
            }

            info!("Added {added} message(s)");
        }

        info!("Done, {added} message(s) added");

        Ok(())
    }
}

impl<C> App<C> {
    async fn load_chat(self) -> Result<App<Peer>> {
        let chat = self
            .client
            .resolve_username(&self.config.chat_name)
            .await?
            .ok_or_else(|| eyre!("Failed to resolve chat name {}", self.config.chat_name))?;

        Ok(App {
            chat,
            updates: self.updates,
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

    #[serde(default)]
    pub force_repopulate: bool,
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("data dir cannot be found")
        .join("realmkbot")
}

impl Config {
    pub fn load() -> Result<Self> {
        use figment::{
            Figment,
            providers::{Env, Format, Json, Toml},
        };

        dotenvy::dotenv().ok();
        let config_dir = dirs::config_dir()
            .context("Config dir cannot be found")?
            .join("realmkbot");

        info!("Config dir: {}", config_dir.display());

        Figment::new()
            .merge(Json::file(config_dir.join("config.json")))
            .merge(Toml::file(config_dir.join("config.toml")))
            .merge(Json::file("config.json"))
            .merge(Toml::file("config.toml"))
            .merge(Env::raw())
            .extract()
            .context("Failed to load config")
    }

    pub fn tdlib_dir(&self) -> PathBuf {
        self.data_dir.join("tdlib")
    }
}
