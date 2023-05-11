mod structures;

use clap::Parser;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};

const MAX_FRAME_SIZE: usize = 256;

lazy_static! {
    static ref REMOTE_SENDERS: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> = Arc::new(Mutex::new(HashMap::new()));

    static ref TUNN_SENDERS: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> = Arc::new(Mutex::new(HashMap::new()));

    static ref TUNNS_PER_CLIENT: Arc<Mutex<HashMap<u32, Vec<u32>>>> = Arc::new(Mutex::new(HashMap::new()));

    // connection_id -> seq -> data
    static ref FUTURE_DATA: Arc<Mutex<HashMap<u32, HashMap<u32, Vec<u8>>>>> = Arc::new(Mutex::new(HashMap::new()));

    // connection_id -> seq
    static ref LAST_SEQ: Arc<Mutex<HashMap<u32, u32>>> = Arc::new(Mutex::new(HashMap::new()));
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

    let listener = TcpListener::bind((args.listen, args.port)).await.unwrap();
    while let Ok((client_socket, _)) = listener.accept().await {
        let (sender_tx, sender_rx) = mpsc::channel(100);

        let tunn_id = rand::random::<u32>();
        TUNN_SENDERS.lock().unwrap().insert(tunn_id, sender_tx);

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

    let frame_type = structures::get_frame_type(frame_type_num);
    if !auth_success && !matches!(frame_type, structures::FrameType::Auth) {
        panic!("auth required")
    }
    if matches!(frame_type, structures::FrameType::Auth) {
        let frame = structures::FrameAuth::from_bytes(&data);
        println!("auth recv[{:?}]: {:?}", tunn_id, frame);
        auth_success = true;
        client_id = frame.client_id;
        let mut tunns_per_client_unlocked = TUNNS_PER_CLIENT.lock().unwrap();
        if !tunns_per_client_unlocked.contains_key(&frame.client_id) {
            tunns_per_client_unlocked.insert(frame.client_id, vec![]);
        }
        tunns_per_client_unlocked
            .get_mut(&frame.client_id)
            .unwrap()
            .push(tunn_id);
        return Ok((auth_success, client_id));
    }
    if matches!(frame_type, structures::FrameType::Close) {
        let frame = structures::FrameClose::from_bytes(&data);
        println!("close recv[{:?}]: {:?}", tunn_id, frame);
        let connection_id = frame.connection_id;

        let remote_tx = REMOTE_SENDERS
            .lock()
            .unwrap()
            .get(&connection_id)
            .unwrap()
            .clone();
        remote_tx.send(vec![]).await.unwrap();

        let mut future_data_unlocked = FUTURE_DATA.lock().unwrap();
        future_data_unlocked.remove(&connection_id);

        let mut last_seq_unlocked = LAST_SEQ.lock().unwrap();
        last_seq_unlocked.remove(&connection_id);

        return Ok((auth_success, client_id));
    }

    if matches!(frame_type, structures::FrameType::Bind) {
        let frame = structures::FrameBind::from_bytes(&data);
        println!("bind recv[{:?}]: {:?}", tunn_id, frame);
        let is_some_send_before = LAST_SEQ.lock().unwrap().contains_key(&frame.connection_id);
        if is_some_send_before {
            panic!("bind request for already binded connection")
        }
        create_remote_conn(
            frame.connection_id,
            frame.dest_host,
            frame.dest_port,
            client_id,
        )
        .await?;
        LAST_SEQ
            .lock()
            .unwrap()
            .insert(frame.connection_id, frame.seq);
        return Ok((auth_success, client_id));
    }

    if matches!(frame_type, structures::FrameType::Data) {
        let frame = structures::FrameData::from_bytes(&data);
        println!(
            "data recv[{:?}]: conn_id: {}, seq: {}",
            tunn_id, frame.connection_id, frame.seq
        );
        let connection_id = frame.connection_id;
        let seq = frame.seq;

        let is_some_send_before = LAST_SEQ.lock().unwrap().contains_key(&connection_id);
        let last_send_seq = {
            if is_some_send_before {
                LAST_SEQ
                    .lock()
                    .unwrap()
                    .get(&connection_id)
                    .unwrap()
                    .clone()
            } else {
                0
            }
        };

        let mut should_send = false;
        if is_some_send_before && seq == last_send_seq + 1 {
            should_send = true;
        }

        if should_send {
            let remote_tx = {
                let remote_senders_unlocked = REMOTE_SENDERS.lock().unwrap();
                remote_senders_unlocked.get(&connection_id).unwrap().clone()
            };
            remote_tx.send(frame.data).await?;
            LAST_SEQ.lock().unwrap().insert(connection_id, seq);
            let mut next_seq = seq + 1;
            loop {
                let mut future_data_unlocked = FUTURE_DATA.lock().unwrap().clone();
                let future_data_conn = future_data_unlocked
                    .entry(connection_id)
                    .or_insert(HashMap::new());
                if future_data_conn.contains_key(&next_seq) {
                    let remote_tx = {
                        let remote_senders_unlocked = REMOTE_SENDERS.lock().unwrap();
                        remote_senders_unlocked.get(&connection_id).unwrap().clone()
                    };
                    let data = future_data_conn.get(&next_seq).unwrap().clone();
                    remote_tx.send(data).await?;
                    LAST_SEQ.lock().unwrap().insert(connection_id, next_seq);
                    future_data_conn.remove(&next_seq);
                    next_seq = next_seq + 1;
                    continue;
                }
                break;
            }
        } else {
            let mut future_dat_unlocked = FUTURE_DATA.lock().unwrap();
            let future_data_conn = future_dat_unlocked
                .entry(connection_id)
                .or_insert(HashMap::new());
            future_data_conn.insert(seq, frame.data);
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
    println!("data send: conn_id: {}, seq: {}", connection_id, seq);

    let tunn_tx = {
        let tunns_per_client_unlocked = TUNNS_PER_CLIENT.lock().unwrap();
        let tunn_ids = tunns_per_client_unlocked.get(&client_id).unwrap();
        let tunnel_number = (connection_id + seq) as usize % tunn_ids.len();
        let tunn_id = tunn_ids[tunnel_number];
        let tunn_senders_unlocked = TUNN_SENDERS.lock().unwrap();
        let tunn_tx = tunn_senders_unlocked.get(&tunn_id).unwrap().clone();
        tunn_tx
    };

    tunn_tx.send(frame.to_bytes_with_header().to_vec()).await?;
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
    mut sender_rx: Receiver<Vec<u8>>,
    tunn_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    tokio::spawn(async move {
        if let Err(err) = read_client_loop(socket_reader, tunn_id).await {
            println!("read_client_loop error: {}", err);
        }
    });

    tokio::spawn(async move {
        if let Err(err) = write_client_loop(socket_writer, &mut sender_rx).await {
            println!("write_client_loop error: {}", err);
        }
    });

    Ok(())
}

async fn read_client_loop(
    mut socket_reader: OwnedReadHalf,
    tunn_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_client_loop");
    let mut auth_success = false;
    let mut client_id: u32 = 0;
    loop {
        let magic = socket_reader.read_u16().await?;
        if magic != structures::FRAME_MAGIC {
            dbg!(magic);
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
        match process_client_data(frame_type, data_buf, tunn_id, auth_success, client_id).await {
            Ok((new_success, new_client_id)) => {
                auth_success = new_success;
                client_id = new_client_id;
            }
            Err(err) => {
                println!("process_client_data error: {}", err);
            }
        };
    }
}

async fn write_client_loop(
    mut socket_writer: OwnedWriteHalf,
    sender_rx: &mut Receiver<Vec<u8>>,
) -> io::Result<()> {
    println!("start write_client_loop");
    while let Some(res) = sender_rx.recv().await {
        socket_writer.write_all(&res).await?;
    }
    println!("end write_client_loop");
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

    let (sender_tx, mut sender_rx) = mpsc::channel(100);
    REMOTE_SENDERS
        .lock()
        .unwrap()
        .insert(connection_id, sender_tx);

    tokio::spawn(async move {
        if let Err(err) = read_remote_loop(remote_socket_reader, connection_id, client_id).await {
            println!("read_remote_loop error: {}", err);
        }
    });

    tokio::spawn(async move {
        if let Err(err) = write_remote_loop(remote_socket_writer, &mut sender_rx).await {
            println!("write_remote_loop error: {}", err);
        }
    });
    Ok(())
}

async fn read_remote_loop(
    mut remote_socket_reader: OwnedReadHalf,
    connection_id: u32,
    client_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_remote_loop");
    let mut buf = [0; MAX_FRAME_SIZE];
    let mut seq = 0;
    loop {
        let len = remote_socket_reader.read(&mut buf).await?;
        if len == 0 {
            break;
        }
        process_remote_data(buf, len, connection_id, seq, client_id).await?;
        seq = seq + 1;
    }
    println!("end read_remote_loop");
    Ok(())
}

async fn write_remote_loop(
    mut remote_socket_writer: OwnedWriteHalf,
    sender_rx: &mut Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start write_remote_loop");
    while let Some(buf) = sender_rx.recv().await {
        if buf.len() == 0 {
            remote_socket_writer.shutdown().await?;
            break;
        }
        remote_socket_writer.write_all(&buf).await?;
    }
    println!("end write_remote_loop");
    Ok(())
}
