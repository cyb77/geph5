use anyhow::Context;
use geph5_broker_protocol::{BridgeDescriptor, RouteDescriptor};
use geph5_misc_rpc::bridge::{B2eMetadata, BridgeControlClient, ObfsProtocol};
use moka::future::Cache;
use nanorpc_sillad::DialerTransport;
use once_cell::sync::Lazy;
use sillad::tcp::TcpDialer;
use sillad_sosistab3::{dialer::SosistabDialer, Cookie};
use smol_timeout2::TimeoutExt;
use std::{
    net::SocketAddr,
    time::{Duration, SystemTime},
};

pub async fn bridge_to_leaf_route(
    bridge: BridgeDescriptor,
    delay_ms: u32,
    exit_b2e: SocketAddr,
) -> anyhow::Result<RouteDescriptor> {
    static CACHE: Lazy<Cache<(SocketAddr, SocketAddr), RouteDescriptor>> = Lazy::new(|| {
        Cache::builder()
            .time_to_live(Duration::from_secs(60))
            .build()
    });

    let cookie = Cookie::new(&bridge.control_cookie);

    CACHE
        .try_get_with((bridge.control_listen, exit_b2e), async {
            let dialer = SosistabDialer {
                inner: TcpDialer {
                    dest_addr: bridge.control_listen,
                },
                cookie,
            };
            let cookie = format!("exit-cookie-{}", rand::random::<u128>());
            let control_client = BridgeControlClient(DialerTransport(dialer));
            let forwarded_listen = control_client
                .tcp_forward(
                    exit_b2e,
                    B2eMetadata {
                        protocol: ObfsProtocol::Sosistab3(cookie.clone()),
                        expiry: SystemTime::now() + Duration::from_secs(86400),
                    },
                )
                .timeout(Duration::from_secs(1))
                .await
                .context("timeout")??;
            let no_delay_route = RouteDescriptor::Sosistab3 {
                cookie,
                lower: RouteDescriptor::Tcp(forwarded_listen).into(),
            };
            anyhow::Ok(if delay_ms > 0 {
                RouteDescriptor::Delay {
                    milliseconds: delay_ms,
                    lower: no_delay_route.into(),
                }
            } else {
                no_delay_route
            })
        })
        .await
        .map_err(|err| anyhow::anyhow!("bridge comms failed: {:?}", err))
}
