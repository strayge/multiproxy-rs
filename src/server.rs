mod collections;
mod logging;
mod socket;
mod structures;
use crate::collections::{StorageId, StorageList, StorageSender, StorageSeqData};
use crate::structures::Frame;
use clap::Parser;
use lazy_static::lazy_static;
use log::{debug, error, info};
use tokio::io::AsyncReadExt;
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self};
use tokio_util::sync::CancellationToken;

const MAX_FRAME_SIZE: usize = 4096;

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
    // connection_id -> seq
    static ref LAST_SEND_SEQ: StorageId = StorageId::new();

}

#[derive(Parser)]
#[command(long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "127.0.0.1")]
    listen: String,

    #[arg(short, long, default_value = "5050")]
    port: u16,

    #[arg(long)]
    debug: bool,

    #[arg(long)]
    info: bool,

    #[arg(long)]
    trace: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    logging::config_logging(args.info, args.debug, args.trace)?;

    let listener = TcpListener::bind((args.listen, args.port)).await.unwrap();
    while let Ok((client_socket, _)) = listener.accept().await {
        let (sender_tx, mut sender_rx) = mpsc::channel(100);

        let tunn_id = rand::random::<u32>();
        TUNN_SENDERS.insert(tunn_id, sender_tx).await;

        let (socket_reader, socket_writer) = client_socket.into_split();
        socket::create_socket_coroutines!(
            "client_loop",
            read_client_loop(socket_reader, tunn_id),
            socket::write_loop("client_loop", socket_writer, &mut sender_rx),
            {
                error!("client_loop exit");
            }
        );
    }
    Ok(())
}

async fn process_client_data(
    frame_type_num: u16,
    data: Vec<u8>,
    tunnel_id: u32,
    mut auth_success: bool,
    mut client_id: u32,
) -> Result<(bool, u32), Box<dyn std::error::Error>> {
    // read from tunnel client and pass to remotes

    let frame_type = structures::FrameType::from_number(frame_type_num);
    if !matches!(frame_type, structures::FrameType::Auth) && !auth_success {
        panic!("auth miss");
    }
    let (connection_id, seq, data) = match frame_type {
        structures::FrameType::Auth => {
            let frame = structures::FrameAuth::from_bytes(&data);
            info!("auth recv[{:?}]: {:?}", tunnel_id, frame);
            auth_success = true;
            client_id = frame.client_id;
            TUNNS_PER_CLIENT.insert(frame.client_id, tunnel_id).await;
            return Ok((auth_success, client_id));
        }
        structures::FrameType::Bind => {
            let frame = structures::FrameBind::from_bytes(&data);
            info!("bind recv[{:?}]: {:?}", tunnel_id, frame);
            if LAST_SEQ.contains_key(frame.connection_id).await {
                panic!("second bind request");
            }
            create_remote_conn(frame.connection_id, frame.dest_host, frame.dest_port, client_id)
                .await?;
            LAST_SEQ.insert(frame.connection_id, frame.seq).await;
            return Ok((auth_success, client_id));
        }
        structures::FrameType::Close => {
            let frame = structures::FrameClose::from_bytes(&data);
            info!("close recv[{:?}]: {:?}", tunnel_id, frame);
            (frame.connection_id, frame.seq, vec![])
        }
        structures::FrameType::Data => {
            let frame = structures::FrameData::from_bytes(&data);
            info!(
                "data recv[{:?}]: conn_id: {}, seq: {}",
                tunnel_id, frame.connection_id, frame.seq
            );
            (frame.connection_id, frame.seq, frame.data)
        }
    };

    // common code for frames with data

    let last_seq_internal = LAST_SEQ.storage();
    // save this lock until the end of processing data or deadlock will occur
    let mut last_seq_locked = last_seq_internal.lock().await;
    let is_some_send_before = last_seq_locked.contains_key(&connection_id);
    let mut last_send_seq: u32 = 0;
    if is_some_send_before {
        last_send_seq = last_seq_locked.get(&connection_id).cloned().unwrap();
    }
    let mut should_send = false;
    if is_some_send_before && seq == last_send_seq + 1 {
        should_send = true;
    }

    if should_send {
        REMOTE_SENDERS.send(connection_id, data).await?;
        last_seq_locked.insert(connection_id, seq);
        let mut next_seq = seq + 1;
        loop {
            if FUTURE_DATA.contains_seq(connection_id, next_seq).await {
                let data = FUTURE_DATA.get(connection_id, next_seq).await.unwrap().clone();
                REMOTE_SENDERS.send(connection_id, data).await?;
                last_seq_locked.insert(connection_id, next_seq);
                FUTURE_DATA.remove_seq(connection_id, next_seq).await;
                next_seq += 1;
                continue;
            }
            break;
        }
    } else {
        FUTURE_DATA.insert(connection_id, seq, data).await;
    }
    Ok((auth_success, client_id))
}

async fn process_remote_data(
    data: [u8; MAX_FRAME_SIZE],
    len: usize,
    connection_id: u32,
    seq: u32,
    client_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    // read from remote and pass to tunnel client
    // not called on connection close

    let frame = structures::FrameData {
        connection_id,
        seq,
        length: len as u32,
        data: data[..len].to_vec(),
    };
    info!("data send: conn_id: {}, seq: {}", connection_id, seq);
    let tunn_id = TUNNS_PER_CLIENT.get_randomized(client_id, connection_id + seq).await;
    TUNN_SENDERS.send(tunn_id.unwrap(), frame.to_bytes_with_header()).await?;
    LAST_SEND_SEQ.insert(connection_id, seq).await;
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
        let frame_type = socket_reader.read_u16().await.expect("read frame type error");
        let data_length = socket_reader.read_u16().await.expect("read frame data length error");
        let mut data_buf = vec![0; data_length as usize];
        socket_reader.read_exact(&mut data_buf).await.expect("read frame data error");
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

async fn create_remote_conn(
    connection_id: u32,
    hostname: String,
    port: u16,
    client_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let tunnel = TcpStream::connect(format!("{}:{}", hostname, port)).await?;
    let (remote_socket_reader, remote_socket_writer) = tunnel.into_split();

    let (sender_tx, mut sender_rx) = mpsc::channel(100);

    REMOTE_SENDERS.insert(connection_id, sender_tx).await;

    socket::create_socket_coroutines!(
        "tunnel_loop",
        read_remote_loop(remote_socket_reader, connection_id, client_id),
        socket::write_loop("tunnel_loop", remote_socket_writer, &mut sender_rx),
        {
            // connection closed somehow
            let last_seq = LAST_SEND_SEQ.get(connection_id).await.unwrap_or(0);
            let frame = structures::FrameClose { connection_id, seq: last_seq + 1 };
            let tunn_id = TUNNS_PER_CLIENT.get_randomized(client_id, 0).await.unwrap();
            info!("close send(?): {:?}", frame);
            TUNN_SENDERS.send(tunn_id, frame.to_bytes_with_header()).await.ok();
            REMOTE_SENDERS.remove(connection_id).await;
            FUTURE_DATA.remove(connection_id).await;
            LAST_SEQ.remove(connection_id).await;
            LAST_SEND_SEQ.remove(connection_id).await;
        }
    );
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
        seq += 1;
    }
    info!("end read_remote_loop");
    Ok(())
}
