use std::{
    convert::Infallible,
    sync::{Arc, LazyLock, OnceLock},
    time::{Duration, SystemTime},
};

use anyctx::AnyCtx;
use async_trait::async_trait;
use geph5_broker_protocol::{
    puzzle::solve_puzzle, AccountLevel, ExitDescriptor, ExitList, NewsItem,
};

use itertools::Itertools;
use moka::future::Cache;
use nanorpc::{nanorpc_derive, JrpcRequest, JrpcResponse, RpcService, RpcTransport};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use slab::Slab;

use crate::{broker_client, client::CtxField, logs::LOGS, stats::stat_get_num, Config};

#[nanorpc_derive]
#[async_trait]
pub trait ControlProtocol {
    async fn conn_info(&self) -> ConnInfo;
    async fn stat_num(&self, stat: String) -> f64;
    async fn start_time(&self) -> SystemTime;
    async fn stop(&self);

    async fn recent_logs(&self) -> Vec<String>;

    // broker-proxying stuff

    async fn check_secret(&self, secret: String) -> Result<bool, String>;
    async fn user_info(&self, secret: String) -> Result<UserInfo, String>;
    async fn start_registration(&self) -> Result<usize, String>;
    async fn poll_registration(&self, idx: usize) -> Result<RegistrationProgress, String>;
    async fn stat_history(&self, stat: String) -> Result<Vec<f64>, String>;
    async fn exit_list(&self) -> Result<Vec<ExitDescriptor>, String>;

    async fn latest_news(&self, lang: String) -> Result<Vec<NewsItem>, String>;
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "state")]
pub enum ConnInfo {
    Connecting,
    Connected(ConnectedInfo),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConnectedInfo {
    pub protocol: String,
    pub bridge: String,

    pub exit: ExitDescriptor,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserInfo {
    pub level: AccountLevel,
    pub expiry: Option<u64>,
}

pub struct ControlProtocolImpl {
    pub ctx: AnyCtx<Config>,
}

pub static CURRENT_CONN_INFO: CtxField<Mutex<ConnInfo>> = |_| Mutex::new(ConnInfo::Connecting);

static REGISTRATIONS: LazyLock<Mutex<Slab<RegistrationProgress>>> =
    LazyLock::new(|| Mutex::new(Slab::new()));

#[derive(Serialize, Deserialize, Clone)]
pub struct RegistrationProgress {
    pub progress: f64,
    pub secret: Option<String>,
}

#[async_trait]
impl ControlProtocol for ControlProtocolImpl {
    async fn conn_info(&self) -> ConnInfo {
        self.ctx.get(CURRENT_CONN_INFO).lock().clone()
    }

    async fn stat_num(&self, stat: String) -> f64 {
        stat_get_num(&self.ctx, &stat)
    }

    async fn start_time(&self) -> SystemTime {
        static START_TIME: CtxField<SystemTime> = |_| SystemTime::now();
        *self.ctx.get(START_TIME)
    }

    async fn stop(&self) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::process::exit(0);
        });
    }

    async fn recent_logs(&self) -> Vec<String> {
        let logs = LOGS.lock();
        String::from_utf8_lossy(&logs)
            .split('\n')
            .map(|s| s.to_string())
            .collect_vec()
    }

    async fn check_secret(&self, secret: String) -> Result<bool, String> {
        let res = broker_client(&self.ctx)
            .map_err(|e| format!("{:?}", e))?
            .get_user_info_by_cred(geph5_broker_protocol::Credential::Secret(secret))
            .await
            .map_err(|e| format!("{:?}", e))?
            .map_err(|e| format!("{:?}", e))?;
        Ok(res.is_some())
    }

    async fn user_info(&self, secret: String) -> Result<UserInfo, String> {
        static USER_INFO_CACHE: CtxField<Cache<String, UserInfo>> = |_| {
            Cache::builder()
                .time_to_live(Duration::from_secs(60))
                .build()
        };

        let cache = self.ctx.get(USER_INFO_CACHE);

        cache
            .try_get_with(secret.clone(), async {
                let res = broker_client(&self.ctx)
                    .map_err(|e| format!("{:?}", e))?
                    .get_user_info_by_cred(geph5_broker_protocol::Credential::Secret(secret))
                    .await
                    .map_err(|e| format!("{:?}", e))?
                    .map_err(|e| format!("{:?}", e))?
                    .ok_or_else(|| "no such user".to_string())?;
                Ok(UserInfo {
                    level: if res.plus_expires_unix.is_some() {
                        AccountLevel::Plus
                    } else {
                        AccountLevel::Free
                    },
                    expiry: res.plus_expires_unix,
                })
            })
            .await
            .map_err(|s: Arc<String>| (*s).clone())
    }

    async fn start_registration(&self) -> Result<usize, String> {
        let (puzzle, difficulty) = broker_client(&self.ctx)
            .map_err(|e| format!("{:?}", e))?
            .get_puzzle()
            .await
            .map_err(|e| format!("{:?}", e))?;
        tracing::debug!(puzzle, difficulty, "got puzzle");
        let idx = REGISTRATIONS.lock().insert(RegistrationProgress {
            progress: 0.0,
            secret: None,
        });
        let ctx = self.ctx.clone();
        smolscale::spawn(async move {
            loop {
                let fallible = async {
                    let solution = {
                        let puzzle = puzzle.clone();
                        smol::unblock(move || {
                            solve_puzzle(&puzzle, difficulty, |progress| {
                                REGISTRATIONS.lock()[idx] = RegistrationProgress {
                                    progress,
                                    secret: None,
                                }
                            })
                        })
                        .await
                    };
                    let secret = broker_client(&ctx)?
                        .register_user_secret(puzzle.clone(), solution)
                        .await?
                        .map_err(|e| anyhow::anyhow!(e))?;
                    REGISTRATIONS.lock()[idx] = RegistrationProgress {
                        progress: 1.0,
                        secret: Some(secret.clone()),
                    };
                    anyhow::Ok(secret)
                };
                if let Err(err) = fallible.await {
                    tracing::warn!(err = debug(err), "restarting registration")
                } else {
                    break;
                }
            }
        })
        .detach();
        Ok(idx)
    }

    async fn poll_registration(&self, idx: usize) -> Result<RegistrationProgress, String> {
        tracing::debug!(idx, "polling registration");
        let registers = REGISTRATIONS.lock();
        registers
            .get(idx)
            .cloned()
            .ok_or_else(|| "no such registration".to_string())
    }

    async fn stat_history(&self, stat: String) -> Result<Vec<f64>, String> {
        Ok(vec![1.0, 2.0, 3.0])
    }

    async fn exit_list(&self) -> Result<Vec<ExitDescriptor>, String> {
        let resp = broker_client(&self.ctx)
            .map_err(|e| format!("{:?}", e))?
            .get_exits()
            .await
            .map_err(|e| format!("{:?}", e))?
            .map_err(|e| format!("{:?}", e))?;
        Ok(resp.inner.all_exits.iter().map(|s| s.1.clone()).collect())
    }

    async fn latest_news(&self, lang: String) -> Result<Vec<NewsItem>, String> {
        let client = broker_client(&self.ctx).map_err(|e| format!("{:?}", e))?;
        Ok(client
            .get_news(lang)
            .await
            .map_err(|s| s.to_string())?
            .map_err(|s| s.to_string())?)
    }
}

pub struct DummyControlProtocolTransport(pub ControlService<ControlProtocolImpl>);

#[async_trait]
impl RpcTransport for DummyControlProtocolTransport {
    type Error = Infallible;

    async fn call_raw(&self, req: JrpcRequest) -> Result<JrpcResponse, Self::Error> {
        Ok(self.0.respond_raw(req).await)
    }
}
