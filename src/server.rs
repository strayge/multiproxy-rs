mod collections;
mod logging;
mod structures;
use crate::collections::{StorageId, StorageList, StorageSender, StorageSeqData};
use crate::structures::Frame;
use clap::Parser;
use lazy_static::lazy_static;
use log::{debug, error, info};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver};
use tokio_util::sync::CancellationToken;

const MAX_FRAME_SIZE: usize = 256;

lazy_static! {
    // connection_id -> sender
    static ref REMOTE_SENDERS: StorageSender = StorageSender::new();
    // tunn_id -> sender
    static ref TUNN_SENDERS: StorageSender = StorageSender::new();
    // client_id -> tunn_id
    static ref TUNNS_PER_CLIENT: StorageList = StorageList::new();
    // connection_id -> seq -> data
    static ref FUTURE_DATA: StorageSeqData = StorageSeqData::new();
    // connection_id -> seq
    static ref LAST_SEQ: StorageId = StorageId::new();
}

#[derive(Parser)]
#[command(long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "127.0.0.1")]
    listen: String,

    #[arg(short, long, default_value = "5050")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    logging::config_logging()?;

    let listener = TcpListener::bind((args.listen, args.port)).await.unwrap();
    while let Ok((client_socket, _)) = listener.accept().await {
        let (sender_tx, sender_rx) = mpsc::channel(100);

        let tunn_id = rand::random::<u32>();
        TUNN_SENDERS.insert(tunn_id, sender_tx).await;

        tokio::spawn(async move {
            handle_client_connection(client_socket, sender_rx, tunn_id)
                .await
                .unwrap();
        });
    }
    Ok(())
}

async fn process_client_data(
    frame_type_num: u16,
    data: Vec<u8>,
    tunn_id: u32,
    mut auth_success: bool,
    mut client_id: u32,
) -> Result<(bool, u32), Box<dyn std::error::Error>> {
    // read from tunnel client and pass to remotes

    let frame_type = structures::FrameType::from_number(frame_type_num);
    if !auth_success && !matches!(frame_type, structures::FrameType::Auth) {
        panic!("auth required")
    }
    if matches!(frame_type, structures::FrameType::Auth) {
        let frame = structures::FrameAuth::from_bytes(&data);
        info!("auth recv[{:?}]: {:?}", tunn_id, frame);
        auth_success = true;
        client_id = frame.client_id;
        TUNNS_PER_CLIENT.insert(frame.client_id, tunn_id).await;
        return Ok((auth_success, client_id));
    }
    if matches!(frame_type, structures::FrameType::Close) {
        let frame = structures::FrameClose::from_bytes(&data);
        info!("close recv[{:?}]: {:?}", tunn_id, frame);
        let connection_id = frame.connection_id;
        info!("close send(?): {:?}", frame);
        TUNN_SENDERS.send(tunn_id, frame.to_bytes_with_header()).await.ok();
        REMOTE_SENDERS.send(connection_id, vec![]).await?;
        FUTURE_DATA.remove(connection_id).await;
        LAST_SEQ.remove(connection_id).await;
        return Ok((auth_success, client_id));
    }

    if matches!(frame_type, structures::FrameType::Bind) {
        let frame = structures::FrameBind::from_bytes(&data);
        info!("bind recv[{:?}]: {:?}", tunn_id, frame);
        if LAST_SEQ.contains_key(frame.connection_id).await {
            panic!("bind request for already binded connection")
        }
        create_remote_conn(
            frame.connection_id,
            frame.dest_host,
            frame.dest_port,
            client_id,
        )
        .await?;
        LAST_SEQ.insert(frame.connection_id, frame.seq).await;
        return Ok((auth_success, client_id));
    }

    if matches!(frame_type, structures::FrameType::Data) {
        let frame = structures::FrameData::from_bytes(&data);
        info!(
            "data recv[{:?}]: conn_id: {}, seq: {}",
            tunn_id, frame.connection_id, frame.seq
        );
        let connection_id = frame.connection_id;
        let seq = frame.seq;

        let is_some_send_before = LAST_SEQ.contains_key(connection_id).await;
        let mut last_send_seq: u32 = 0;
        if is_some_send_before {
            last_send_seq = LAST_SEQ.get(connection_id).await.unwrap();
        }

        let mut should_send = false;
        if is_some_send_before && seq == last_send_seq + 1 {
            should_send = true;
        }

        if should_send {
            REMOTE_SENDERS.send(connection_id, frame.data).await?;
            LAST_SEQ.insert(connection_id, seq).await;
            let mut next_seq = seq + 1;
            loop {
                if FUTURE_DATA.contains_seq(connection_id, next_seq).await {
                    let data = FUTURE_DATA
                        .get(connection_id, next_seq)
                        .await
                        .unwrap()
                        .clone();
                    REMOTE_SENDERS.send(connection_id, data).await?;
                    LAST_SEQ.insert(connection_id, next_seq).await;
                    FUTURE_DATA.remove_seq(connection_id, next_seq).await;
                    next_seq = next_seq + 1;
                    continue;
                }
                break;
            }
        } else {
            FUTURE_DATA.insert(connection_id, seq, frame.data).await;
        }
        return Ok((auth_success, client_id));
    }

    panic!("unknown frame type")
}

async fn process_remote_data(
    data: [u8; MAX_FRAME_SIZE],
    len: usize,
    connection_id: u32,
    seq: u32,
    client_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    // read from remote and pass to tunnel client

    let frame = structures::FrameData {
        connection_id: connection_id,
        seq: seq,
        length: len as u32,
        data: data[..len].to_vec(),
    };
    info!("data send: conn_id: {}, seq: {}", connection_id, seq);
    let tunn_id = TUNNS_PER_CLIENT
        .get_randomized(client_id, connection_id + seq)
        .await;
    TUNN_SENDERS
        .send(tunn_id.unwrap(), frame.to_bytes_with_header())
        .await?;
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
    mut sender_rx: Receiver<Vec<u8>>,
    tunn_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    let token = CancellationToken::new();
    let token2 = token.clone();

    tokio::spawn(async move {
        tokio::select! {
            res = read_client_loop(socket_reader, tunn_id) => {
                if res.is_err() {
                    error!("read_client_loop error: {}", res.err().unwrap());
                }
                else {
                    info!("read_client_loop end");
                }
                token.cancel();
            }
            _ = token.cancelled() => {
                info!("read_client_loop cancelled");
            }
        }
    });

    tokio::spawn(async move {
        tokio::select! {
            res = write_client_loop(socket_writer, &mut sender_rx) => {
                if res.is_err() {
                    error!("write_client_loop error: {}", res.err().unwrap());
                }
                else {
                    info!("read_client_loop end");
                }
                token2.cancel();
            }
            _ = token2.cancelled() => {
                info!("write_client_loop cancelled");
            }
        }
    });

    Ok(())
}

async fn read_client_loop(
    mut socket_reader: OwnedReadHalf,
    tunn_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start read_client_loop");
    let mut auth_success = false;
    let mut client_id: u32 = 0;
    loop {
        let magic = socket_reader.read_u16().await?;
        if magic != structures::FRAME_MAGIC {
            panic!("invalid magic");
        }
        let frame_type = socket_reader
            .read_u16()
            .await
            .expect("read frame type error");
        let data_length = socket_reader
            .read_u16()
            .await
            .expect("read frame data length error");
        let mut data_buf = vec![0; data_length as usize];
        socket_reader
            .read_exact(&mut data_buf)
            .await
            .expect("read frame data error");
        debug!("client_socket read len={}", data_buf.len());
        match process_client_data(frame_type, data_buf, tunn_id, auth_success, client_id).await {
            Ok((new_success, new_client_id)) => {
                auth_success = new_success;
                client_id = new_client_id;
            }
            Err(err) => {
                error!("process_client_data error: {}", err);
            }
        };
    }
}

async fn write_client_loop(
    mut socket_writer: OwnedWriteHalf,
    sender_rx: &mut Receiver<Vec<u8>>,
) -> io::Result<()> {
    info!("start write_client_loop");
    while let Some(res) = sender_rx.recv().await {
        socket_writer.write_all(&res).await?;
        debug!("client_socket write len={}", res.len());
    }
    info!("end write_client_loop");
    Ok(())
}

async fn create_remote_conn(
    connection_id: u32,
    hostname: String,
    port: u16,
    client_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let tunnel = TcpStream::connect(format!("{}:{}", hostname, port)).await?;
    let (remote_socket_reader, remote_socket_writer) = tunnel.into_split();

    let (mut sender_tx, mut sender_rx) = mpsc::channel(100);

    REMOTE_SENDERS.insert(connection_id, sender_tx).await;

    let token = CancellationToken::new();
    let token2 = token.clone();

    tokio::spawn(async move {
        tokio::select! {
            res = read_remote_loop(remote_socket_reader, connection_id, client_id) => {
                if res.is_err() {
                    info!("read_remote_loop error: {}", res.err().unwrap());
                }
                else {
                    info!("read_remote_loop end");
                }
                token.cancel();
            }
            _ = token.cancelled() => {
                info!("read_remote_loop cancelled");
            }
        }
    });

    tokio::spawn(async move {
        tokio::select! {
            res = write_remote_loop(remote_socket_writer, &mut sender_rx) => {
                if res.is_err() {
                    info!("write_remote_loop error: {}", res.err().unwrap());
                }
                else {
                    info!("write_remote_loop end");
                }
                token2.cancel();
            }
            _ = token2.cancelled() => {
                info!("write_remote_loop cancelled");
            }
        }
        let frame = structures::FrameClose { connection_id: connection_id, seq: 0 };
        let tunn_id = TUNNS_PER_CLIENT.get_randomized(client_id, 0).await.unwrap();
        info!("close send(?): {:?}", frame);
        TUNN_SENDERS.send(tunn_id, frame.to_bytes_with_header()).await.ok();
        REMOTE_SENDERS.remove(connection_id).await;
        FUTURE_DATA.remove(connection_id).await;
        LAST_SEQ.remove(connection_id).await;
    });

    Ok(())
}

async fn read_remote_loop(
    mut remote_socket_reader: OwnedReadHalf,
    connection_id: u32,
    client_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start read_remote_loop");
    let mut buf = [0; MAX_FRAME_SIZE];
    let mut seq = 0;
    loop {
        let len = remote_socket_reader.read(&mut buf).await?;
        debug!("remote_socket read len={}", len);
        if len == 0 {
            break;
        }
        process_remote_data(buf, len, connection_id, seq, client_id).await?;
        seq = seq + 1;
    }
    info!("end read_remote_loop");
    Ok(())
}

async fn write_remote_loop(
    mut remote_socket_writer: OwnedWriteHalf,
    sender_rx: &mut Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start write_remote_loop");
    while let Some(buf) = sender_rx.recv().await {
        if buf.len() == 0 {
            break;
        }
        remote_socket_writer.write_all(&buf).await?;
        debug!("remote_socket write len={}", buf.len());
    }
    info!("end write_remote_loop");
    Ok(())
}
