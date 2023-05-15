mod collections;
mod logging;
mod socket;
mod socks;
mod structures;
use crate::collections::{StorageId, StorageSender, StorageSeqData};
use crate::structures::Frame;
use clap::Parser;
use lazy_static::lazy_static;
use log::{debug, error, info};

use tokio::io::AsyncReadExt;
use tokio::net::tcp::OwnedReadHalf;
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
    logging::config_logging()?;

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
        let frame = structures::FrameClose { connection_id: connection_id, seq: seq };
        info!("close send: {:?}", frame);
        TUNN_SENDERS.send(tunn_id, frame.to_bytes_with_header()).await?;
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
        TUNN_SENDERS.send(tunn_id, frame.to_bytes_with_header()).await?;
    }

    let frame = structures::FrameData {
        connection_id: connection_id,
        seq: seq,
        length: len as u32,
        data: data[..len].to_vec(),
    };
    info!("data send[{}]: conn_id: {}, seq: {}", tunn_id, connection_id, seq);
    TUNN_SENDERS.send(tunn_id, frame.to_bytes_with_header()).await?;
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
    let (connection_id, seq, data) = match frame_type {
        structures::FrameType::Close => {
            let frame = structures::FrameClose::from_bytes(&data);
            info!("close recv[{:?}]: {:?}", tunnel_id, frame);
            (frame.connection_id, frame.seq, vec![])
        }
        structures::FrameType::Data => {
            let frame = structures::FrameData::from_bytes(&data);
            info!("data recv[{:?}]: conn_id: {}, seq: {}", tunnel_id, frame.connection_id, frame.seq);
            (frame.connection_id, frame.seq, frame.data)
        }
        _ => panic!("unknown frame type"),
    };

    let is_already_closed = !CONN_SENDERS.contains_key(connection_id).await;
    if is_already_closed {
        debug!("already closed, {:?} frame ignored", frame_type);
        return Ok(());
    }

    let last_seq_internal = last_seq.storage();
    // save this lock until the end of processing data or deadlock will occur
    let mut last_seq_locked = last_seq_internal.lock().await;
    let is_some_send_before = last_seq_locked.contains_key(&connection_id);
    let mut last_send_seq: u32 = 0;
    if is_some_send_before {
        last_send_seq = last_seq_locked.get(&connection_id).unwrap().clone();
    }
    let mut should_send = false;
    if !is_some_send_before && seq == 0 {
        should_send = true;
    } else if is_some_send_before && seq == last_send_seq + 1 {
        should_send = true;
    }

    if should_send {
        if let Err(e) = CONN_SENDERS.send(connection_id, data).await {
            error!("queue to client error occurred; error = {:?}", e);
            return Ok(());
        }
        debug!("queue to client conn={} seq={}", connection_id, seq);
        last_seq_locked.insert(connection_id, seq);
        let mut next_seq = seq + 1;
        loop {
            if future_data.contains_seq(connection_id, next_seq).await {
                let data = future_data.get(connection_id, next_seq).await.unwrap().clone();
                CONN_SENDERS.send(connection_id, data).await?;
                debug!("queue to client conn={} seq={}", connection_id, next_seq);
                last_seq_locked.insert(connection_id, next_seq);
                future_data.remove_seq(connection_id, next_seq).await;
                next_seq = next_seq + 1;
                continue;
            }
            debug!("seq {} not found, stop sending", next_seq);
            break;
        }
    } else {
        debug!("postponed data conn={} seq={}", connection_id, seq);
        future_data.insert(connection_id, seq, data).await;
    }
    Ok(())
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

        let frame = structures::FrameAuth { client_id: client_id, key: 1234 };
        info!("auth send[{:?}]: {:?}", i, frame);
        TUNN_SENDERS.send(i as u32, frame.to_bytes_with_header()).await?;

        let last_seq = last_seq.clone();
        let future_data = future_data.clone();

        socket::create_socket_coroutines!(
            "tunnel_loop",
            read_tunnel_loop(tunnel_socket_reader, last_seq, future_data, i),
            socket::write_loop("tunnel_loop", tunnel_socket_writer, &mut sender_rx),
            std::process::exit(1)
        );
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
        let frame_type = tunnel_socket_reader.read_u16().await.expect("error read frame type");
        let data_length =
            tunnel_socket_reader.read_u16().await.expect("error read frame data length");
        let mut data_buf = vec![0; data_length as usize];
        tunnel_socket_reader.read_exact(&mut data_buf).await.expect("error read frame data");
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

async fn handle_client_connection(
    client_socket: TcpStream,
    connection_id: u32,
    mut sender_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut socket_reader, mut socket_writer) = client_socket.into_split();

    // socks5 proxy preable (only connect method without auth)
    // FIXME: is there a way to not pass socket_reader and socket_writer back?
    let (socket_reader, socket_writer, hostname, port) =
        socks::socks5_preamble(socket_reader, socket_writer).await?;

    socket::create_socket_coroutines!(
        "client_loop",
        read_client_loop(socket_reader, hostname, port, connection_id),
        socket::write_loop("client_loop", socket_writer, &mut sender_rx),
        {
            let frame = structures::FrameClose { connection_id: connection_id, seq: 0 };
            info!("close send(?): conn_id: {}", connection_id);
            TUNN_SENDERS.send(0, frame.to_bytes_with_header()).await.ok();
            CONN_SENDERS.remove(connection_id).await;
        }
    );
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
