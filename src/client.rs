mod collections;
mod structures;
use crate::collections::{StorageId, StorageSender, StorageSeqData};
use crate::structures::Frame;
use clap::Parser;
use lazy_static::lazy_static;
use log::{debug, error, info};
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self};
use tokio_util::sync::CancellationToken;

const MAX_FRAME_SIZE: usize = 256;

lazy_static! {
    // send data to actual clients
    static ref CONN_SENDERS: StorageSender = StorageSender::new();
    // send data to to tunnel server
    static ref TUNN_SENDERS: StorageSender = StorageSender::new();
}

#[derive(Parser)]
#[command(long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "127.0.0.1")]
    listen: String,

    #[arg(short, long, default_value = "5000")]
    port: u16,

    #[arg(long, default_value = "127.0.0.1")]
    server_host: String,

    #[arg(long, default_value = "5050")]
    server_port: u16,

    #[arg(short, long, default_value = "2")]
    concurrency: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    simplelog::TermLogger::init(
        log::LevelFilter::Debug,
        simplelog::Config::default(),
        simplelog::TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    )?;

    create_tunnel(args.server_host, args.server_port, args.concurrency).await?;

    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    while let Ok((client_socket, _)) = listener.accept().await {
        let connection_id = rand::random::<u32>();

        let (sender_tx, sender_rx) = mpsc::channel(100);

        CONN_SENDERS.insert(connection_id, sender_tx).await;

        tokio::spawn(async move {
            if let Err(e) = handle_client_connection(client_socket, connection_id, sender_rx).await
            {
                error!("handle_client_connection error occurred; error = {:?}", e);
            }
        });
    }
    info!("main loop end");

    Ok(())
}

async fn process_client_data(
    data: [u8; MAX_FRAME_SIZE],
    len: usize,
    connection_id: u32,
    seq: u32,
    is_first_data: bool,
    is_close: bool,
    hostname: String,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    // read data from client and send to tunnel

    // 1 to desync request and response from using same tunnel
    let offset = connection_id + seq + 1;
    let tunn_id = TUNN_SENDERS.get_randomized_key(offset).await.unwrap();

    if is_close {
        let frame = structures::FrameClose {
            connection_id: connection_id,
            seq: seq,
        };
        info!("close send: {:?}", frame);
        TUNN_SENDERS
            .send(tunn_id, frame.to_bytes_with_header())
            .await?;
        return Ok(());
    }

    if is_first_data {
        let frame = structures::FrameBind {
            connection_id: connection_id,
            seq: 0,
            dest_host: hostname,
            dest_port: port,
        };
        info!("bind send[{:?}]: {:?}", tunn_id, frame);
        TUNN_SENDERS
            .send(tunn_id, frame.to_bytes_with_header())
            .await?;
    }

    let frame = structures::FrameData {
        connection_id: connection_id,
        seq: seq,
        length: len as u32,
        data: data[..len].to_vec(),
    };
    info!(
        "data send[{}]: conn_id: {}, seq: {}",
        tunn_id, connection_id, seq
    );
    TUNN_SENDERS
        .send(tunn_id, frame.to_bytes_with_header())
        .await?;
    Ok(())
}

async fn process_tunnel_data(
    frame_type_num: u16,
    data: Vec<u8>,
    last_seq: StorageId,
    future_data: StorageSeqData,
    tunnel_id: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    // read data from tunnel and send to client

    let frame_type = structures::FrameType::from_number(frame_type_num);
    if matches!(frame_type, structures::FrameType::Close) {
        let frame = structures::FrameClose::from_bytes(&data);
        info!("close recv[{:?}]: {:?}", tunnel_id, frame);
        // close frame received, there are 2 possibilities:
        // 1. we initialized closing, remote connection closed, CONN_SENDERS cleaned up
        // 2. server initialized closing, need full cleanup
        let connection_id = frame.connection_id;
        CONN_SENDERS.send(connection_id, vec![]).await.ok();
        CONN_SENDERS.remove(connection_id).await;
        future_data.remove(connection_id).await;
        last_seq.remove(connection_id).await;
        return Ok(());
    }
    if matches!(frame_type, structures::FrameType::Data) {
        let frame = structures::FrameData::from_bytes(&data);
        info!(
            "data recv[{:?}]: conn_id: {}, seq: {}",
            tunnel_id, frame.connection_id, frame.seq
        );
        let connection_id = frame.connection_id;
        let seq = frame.seq;
        let is_already_closed = !CONN_SENDERS.contains_key(connection_id).await;
        if is_already_closed {
            debug!("already closed, data frame ignore");
            return Ok(());
        }

        let is_some_send_before = last_seq.contains_key(connection_id).await;
        let mut last_send_seq: u32 = 0;
        if is_some_send_before {
            last_send_seq = last_seq.get(connection_id).await.unwrap();
        }

        let mut should_send = false;
        if !is_some_send_before && seq == 0 {
            should_send = true;
        } else if is_some_send_before && seq == last_send_seq + 1 {
            should_send = true;
        }

        if should_send {
            if let Err(e) = CONN_SENDERS.send(connection_id, frame.data).await {
                error!("send to client error occurred; error = {:?}", e);
                return Ok(());
            }
            debug!("send to client conn={} seq={}", connection_id, seq);
            last_seq.insert(connection_id, seq).await;
            let mut next_seq = seq + 1;
            loop {
                if future_data.contains_seq(connection_id, next_seq).await {
                    let data = future_data
                        .get(connection_id, next_seq)
                        .await
                        .unwrap()
                        .clone();
                    CONN_SENDERS.send(connection_id, data).await?;
                    debug!("send to client conn={} seq={}", connection_id, seq);
                    last_seq.insert(connection_id, next_seq).await;
                    future_data.remove_seq(connection_id, next_seq).await;
                    next_seq = next_seq + 1;
                    continue;
                }
                break;
            }
        } else {
            future_data.insert(connection_id, seq, frame.data).await;
        }
        return Ok(());
    }
    panic!("unknown frame type");
}

async fn create_tunnel(
    hostname: String,
    port: u16,
    number: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let client_id = rand::random::<u32>();

    // connection_id -> seq -> data
    let future_data: StorageSeqData = StorageSeqData::new();

    // connection_id -> seq
    let last_seq: StorageId = StorageId::new();

    for i in 0..number {
        let tunnel = TcpStream::connect(format!("{}:{}", hostname, port)).await?;
        let (tunnel_socket_reader, tunnel_socket_writer) = tunnel.into_split();

        let (sender_tx, mut sender_rx) = mpsc::channel(100);
        TUNN_SENDERS.insert(i as u32, sender_tx).await;

        let frame = structures::FrameAuth {
            client_id: client_id,
            key: 1234,
        };
        info!("auth send[{:?}]: {:?}", i, frame);
        TUNN_SENDERS
            .send(i as u32, frame.to_bytes_with_header())
            .await?;

        let last_seq = last_seq.clone();
        let future_data = future_data.clone();

        let token = CancellationToken::new();
        let token2 = token.clone();

        tokio::spawn(async move {
            tokio::select! {
                res = read_tunnel_loop(tunnel_socket_reader, last_seq, future_data, i) => {
                    if res.is_err() {
                        error!("read_tunnel_loop error: {}", res.err().unwrap());
                    }
                    else {
                        info!("read_tunnel_loop end");
                    }
                    token.cancel();
                }
                _ = token.cancelled() => {
                    info!("read_tunnel_loop cancelled");
                }
            }
        });

        tokio::spawn(async move {
            tokio::select! {
                res = write_tunnel_loop(tunnel_socket_writer, &mut sender_rx) => {
                    if res.is_err() {
                        error!("write_tunnel_loop error: {}", res.err().unwrap());
                    }
                    else {
                        info!("write_tunnel_loop end");
                    }
                    token2.cancel();
                }
                _ = token2.cancelled() => {
                    info!("write_tunnel_loop cancelled");
                }
            }
        });
    }
    Ok(())
}

async fn read_tunnel_loop(
    mut tunnel_socket_reader: OwnedReadHalf,
    last_seq: StorageId,
    future_data: StorageSeqData,
    tunnel_id: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start read_tunnel_loop");
    loop {
        let magic = tunnel_socket_reader.read_u16().await?;
        if magic != structures::FRAME_MAGIC {
            panic!("invalid magic");
        }
        let frame_type = tunnel_socket_reader
            .read_u16()
            .await
            .expect("error read frame type");
        let data_length = tunnel_socket_reader
            .read_u16()
            .await
            .expect("error read frame data length");
        let mut data_buf = vec![0; data_length as usize];
        tunnel_socket_reader
            .read_exact(&mut data_buf)
            .await
            .expect("error read frame data");
        debug!("tunnel_socket read len={}", data_buf.len());
        if let Err(e) = process_tunnel_data(
            frame_type,
            data_buf,
            last_seq.clone(),
            future_data.clone(),
            tunnel_id,
        )
        .await
        {
            error!("processing frame error: {}", e);
        }
    }
}

async fn write_tunnel_loop(
    mut tunnel_socket_writer: OwnedWriteHalf,
    sender_rx: &mut mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start write_tunnel_loop");
    while let Some(buf) = sender_rx.recv().await {
        if buf.len() == 0 {
            break;
        }
        tunnel_socket_writer.write_all(&buf).await?;
        debug!("tunnel_socket write len={}", buf.len());
    }
    info!("end write_tunnel_loop");
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
    connection_id: u32,
    mut sender_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut socket_reader, mut socket_writer) = client_socket.into_split();

    // socks5 proxy preable (only connect method without auth)
    let mut init_msg = [0; 2];
    socket_reader.read_exact(&mut init_msg).await?;
    if init_msg[0] != 5 {
        return Err(format!("invalid version[1]: {:?}", init_msg).into());
    }
    let methods_length = init_msg[1] as usize;
    let mut methods = vec![0; methods_length];
    socket_reader.read_exact(&mut methods).await?;

    let init_reply = vec![5, 0];
    socket_writer.write_all(&init_reply).await?;

    let mut bind_msg_start = [0; 4];
    socket_reader.read_exact(&mut bind_msg_start).await?;
    if bind_msg_start[0] != 5 {
        return Err(format!("invalid version[2]: {:?}", bind_msg_start).into());
    }
    if bind_msg_start[1] != 1 {
        let bind_response_failed = vec![5, 7, 0, bind_msg_start[3], 0, 0, 0, 0, 0, 0];
        socket_writer.write_all(&bind_response_failed).await?;
        return Err("unsuppported command".into());
    }
    let hostname;
    let port;
    if bind_msg_start[3] == 3 {
        let mut bind_hostname_len = [0; 1];
        socket_reader.read_exact(&mut bind_hostname_len).await?;
        let mut bind_hostname = vec![0; bind_hostname_len[0] as usize];
        socket_reader.read_exact(&mut bind_hostname).await?;
        let mut bind_port = [0; 2];
        socket_reader.read_exact(&mut bind_port).await?;
        hostname = String::from_utf8(bind_hostname)?;
        port = u16::from_be_bytes(bind_port);
    } else if bind_msg_start[3] == 1 {
        let mut bind_ip = [0; 4];
        socket_reader.read_exact(&mut bind_ip).await?;
        let mut bind_port = [0; 2];
        socket_reader.read_exact(&mut bind_port).await?;
        let ip = IpAddr::V4(Ipv4Addr::from(bind_ip));
        hostname = ip.to_string();
        port = u16::from_be_bytes(bind_port);
    } else {
        return Err("invalid address type".into());
    }
    let bind_response = vec![5, 0, 0, bind_msg_start[3], 0, 0, 0, 0, 0, 0];
    socket_writer.write_all(&bind_response).await?;

    let token = CancellationToken::new();
    let token2 = token.clone();

    tokio::spawn(async move {
        tokio::select! {
            res = read_client_loop(socket_reader, hostname, port, connection_id) => {
                if res.is_err() {
                    info!("read_client_loop error: {}", res.err().unwrap());
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
                    info!("write_client_loop error: {}", res.err().unwrap());
                }
                else {
                    info!("write_client_loop end");
                }
                token2.cancel();
            }
            _ = token2.cancelled() => {
                info!("write_client_loop cancelled");
            }
        }
        // here client connection closed, so clean up
        let frame = structures::FrameClose { connection_id: connection_id, seq: 0 };
        info!("close send(?): conn_id: {}", connection_id);
        TUNN_SENDERS.send(0, frame.to_bytes_with_header()).await.ok();
        CONN_SENDERS.remove(connection_id).await;
    });

    Ok(())
}

async fn read_client_loop(
    mut socket_reader: OwnedReadHalf,
    hostname: String,
    port: u16,
    connection_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("start read_client_loop");
    let mut buf = [0; MAX_FRAME_SIZE];
    let mut is_first_data = true;
    let mut is_close = false;
    let mut seq = 1;

    loop {
        let len = socket_reader.read(&mut buf).await?;
        debug!("client_socket read len={}", buf.len());
        if len == 0 {
            is_close = true;
        }
        process_client_data(
            buf,
            len,
            connection_id,
            seq,
            is_first_data,
            is_close,
            hostname.clone(),
            port,
        )
        .await?;
        seq = seq + 1;
        is_first_data = false;
        if is_close {
            break;
        }
    }
    info!("end read_client_loop");
    Ok(())
}

async fn write_client_loop(
    mut socket_writer: OwnedWriteHalf,
    sender_rx: &mut mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    info!("start write_client_loop");
    while let Some(res) = sender_rx.recv().await {
        if res.len() == 0 {
            break;
        }
        socket_writer.write_all(&res).await?;
        debug!("client_socket write len={}", res.len());
    }
    info!("end write_client_loop");
    Ok(())
}
