use std::io::ErrorKind;

use async_trait::async_trait;
use futures_util::TryFutureExt;
use geph5_misc_rpc::bridge::{B2eMetadata, ObfsProtocol};
use sillad_sosistab3::{listener::SosistabListener, Cookie};
use tachyonix::Receiver;

use super::handle_client;

pub async fn b2e_process(
    b2e_metadata: B2eMetadata,
    recv: Receiver<picomux::Stream>,
) -> anyhow::Result<()> {
    let listener = ReceiverListener(recv);
    match b2e_metadata.protocol {
        ObfsProtocol::Sosistab3(cookie) => {
            b2e_inner(SosistabListener::new(listener, Cookie::new(&cookie))).await
        }
        ObfsProtocol::None => b2e_inner(listener).await,
    }
}

async fn b2e_inner(mut listener: impl sillad::listener::Listener) -> anyhow::Result<()> {
    loop {
        let client = listener.accept().await?;
        smolscale::spawn(
            handle_client(client)
                .map_err(|e| tracing::trace!(err = debug(e), "client stopped through b2e")),
        )
        .detach();
    }
}

struct ReceiverListener(Receiver<picomux::Stream>);

#[async_trait]
impl sillad::listener::Listener for ReceiverListener {
    type P = picomux::Stream;
    async fn accept(&mut self) -> std::io::Result<Self::P> {
        if let Ok(val) = self.0.recv().await {
            Ok(val)
        } else {
            Err(std::io::Error::new(ErrorKind::BrokenPipe, "channel closed"))
        }
    }
}
