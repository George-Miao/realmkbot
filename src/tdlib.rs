use async_trait::async_trait;
use color_eyre::Result;
use rust_tdlib::{
    client::{
        auth_handler::ClientAuthStateHandler, tdlib_client::TdJson, AuthStateHandler, Client,
        ClientIdentifier, Worker,
    },
    tdjson::set_log_verbosity_level,
    types::*,
};
use tokio::{sync::mpsc::Receiver, task::JoinHandle};

use crate::Config;

#[derive(Debug)]
pub struct WorkerHandle {
    _bg: JoinHandle<()>,
    _worker: Worker<BotTokenHandler, TdJson>,
    recv: Receiver<Box<Update>>,
}

impl WorkerHandle {
    // pub async fn join(self) -> Result<(), JoinError> {
    //     self.background.await
    // }

    pub async fn next_update(&mut self) -> Option<Box<Update>> {
        self.recv.recv().await
    }
}

pub async fn init(config: &Config) -> Result<(Client<TdJson>, WorkerHandle)> {
    let mut worker = Worker::builder()
        .with_auth_state_handler(BotTokenHandler {
            bot_token: config.bot_token.clone(),
        })
        .build()?;
    let background = worker.start();

    let tdlib_params = TdlibParameters::builder()
        .database_directory(
            config
                .data_dir
                .join("tdlib")
                .as_os_str()
                .to_str()
                .expect("Non-utf8 path"),
        )
        .system_language_code("en")
        .device_model("Desktop")
        .system_version("0.0")
        .application_version(concat!("realmkbot ", env!("CARGO_PKG_VERSION")))
        .api_id(config.api_id)
        .api_hash(config.api_hash.clone())
        .build();

    // The buffer should be big enough for all initial updates to arrive
    let (send, recv) = tokio::sync::mpsc::channel(1024);

    let client = rust_tdlib::client::Client::builder()
        .with_client_auth_state_handler(BotTokenHandler {
            bot_token: config.bot_token.clone(),
        })
        .with_auth_state_channel(64)
        .with_updates_sender(send)
        .with_tdlib_parameters(tdlib_params)
        .build()?;

    set_log_verbosity_level(1);

    info!("TDLib logging in");

    let client = worker.bind_client(client).await?;

    info!("TDLib logged in");

    client
        .set_log_verbosity_level(
            SetLogVerbosityLevel::builder()
                .new_verbosity_level(1)
                .build(),
        )
        .await?;

    info!("Preparation completed");

    Ok((
        client,
        WorkerHandle {
            _bg: background,
            _worker: worker,
            recv,
        },
    ))
}

#[derive(Debug, Clone)]
pub struct BotTokenHandler {
    bot_token: String,
}

#[async_trait]
impl AuthStateHandler for BotTokenHandler {
    async fn handle_wait_code(
        &self,
        _: Box<dyn ClientAuthStateHandler>,
        _: &AuthorizationStateWaitCode,
    ) -> String {
        info!("handle_wait_code");
        String::new()
    }

    async fn handle_encryption_key(
        &self,
        _: Box<dyn ClientAuthStateHandler>,
        _: &AuthorizationStateWaitEncryptionKey,
    ) -> String {
        info!("handle_encryption_key");
        String::new()
    }

    async fn handle_wait_password(
        &self,
        _: Box<dyn ClientAuthStateHandler>,
        _: &AuthorizationStateWaitPassword,
    ) -> String {
        info!("handle_wait_password");
        String::new()
    }

    async fn handle_wait_client_identifier(
        &self,
        _: Box<dyn ClientAuthStateHandler>,
        _: &AuthorizationStateWaitPhoneNumber,
    ) -> ClientIdentifier {
        info!("handle_wait_client_identifier");
        ClientIdentifier::BotToken(self.bot_token.clone())
    }

    async fn handle_wait_registration(
        &self,
        _: Box<dyn ClientAuthStateHandler>,
        _: &AuthorizationStateWaitRegistration,
    ) -> (String, String) {
        info!("handle_wait_registration");
        (String::new(), String::new())
    }
}

#[async_trait]
impl ClientAuthStateHandler for BotTokenHandler {
    async fn handle_wait_code(&self, _: &AuthorizationStateWaitCode) -> String {
        info!("handle_wait_code");
        String::new()
    }

    async fn handle_encryption_key(&self, _: &AuthorizationStateWaitEncryptionKey) -> String {
        info!("handle_encryption_key");
        String::new()
    }

    async fn handle_wait_password(&self, _: &AuthorizationStateWaitPassword) -> String {
        info!("handle_wait_password");
        String::new()
    }

    async fn handle_wait_client_identifier(
        &self,
        _: &AuthorizationStateWaitPhoneNumber,
    ) -> ClientIdentifier {
        info!("handle_wait_client_identifier");
        ClientIdentifier::BotToken(self.bot_token.clone())
    }

    async fn handle_wait_registration(
        &self,
        _: &AuthorizationStateWaitRegistration,
    ) -> (String, String) {
        info!("handle_wait_registration");
        (String::new(), String::new())
    }
}
