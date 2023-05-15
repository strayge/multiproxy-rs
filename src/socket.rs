use log::{debug, info};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc::{self};

pub async fn write_loop(
    name: &str,
    mut remote_socket_writer: OwnedWriteHalf,
    sender_rx: &mut mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start write_{}", name);
    while let Some(buf) = sender_rx.recv().await {
        if buf.is_empty() {
            break;
        }
        remote_socket_writer.write_all(&buf).await?;
        debug!("{} write len={}", name, buf.len());
    }
    info!("end write_{}", name);
    Ok(())
}

macro_rules! create_socket_coroutines {
    ($name:expr, $read_func:expr, $write_func:expr, $close_actions:expr) => {
        let token = CancellationToken::new();
        let token2 = token.clone();

        tokio::spawn(async move {
            tokio::select! {
                res = $read_func => {
                    if res.is_err() {
                        error!("read_{} error: {}", $name, res.err().unwrap());
                    }
                    else {
                        info!("read_{} end", $name);
                    }
                    token.cancel();
                }
                _ = token.cancelled() => {
                    error!("read_{} cancelled", $name);
                }
            }
        });

        tokio::spawn(async move {
            tokio::select! {
                res = $write_func => {
                    if res.is_err() {
                        error!("write_{} error: {}", $name, res.err().unwrap());
                    }
                    else {
                        info!("write_{} end", $name);
                    }
                    token2.cancel();
                }
                _ = token2.cancelled() => {
                    error!("write_{} cancelled", $name);
                }
            }
            $close_actions;
        });
    };
}

pub(crate) use create_socket_coroutines;
