mod structures;

use clap::Parser;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Sender};

const MAX_FRAME_SIZE: usize = 256;

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

    // send data to actual clients
    let conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // send data to to tunnel server
    let tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    create_tunnel(
        args.server_host,
        args.server_port,
        args.concurrency,
        conn_senders.clone(),
        tunn_senders.clone(),
    )
    .await?;

    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    while let Ok((client_socket, _)) = listener.accept().await {
        let connection_id = rand::random::<u32>();

        let (sender_tx, sender_rx) = mpsc::channel(100);
        conn_senders.lock().unwrap().insert(connection_id, sender_tx);

        let tunn_senders = tunn_senders.clone();
        tokio::spawn(async move {
            handle_client_connection(client_socket, connection_id, sender_rx, tunn_senders)
                .await
                .unwrap();
        });
    }

    Ok(())
}

async fn process_client_data(
    data: [u8; MAX_FRAME_SIZE],
    len: usize,
    connection_id: u32,
    seq: u32,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    is_first_data: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // read data from client and send to tunnel

    let (tunn_id, tunn_sender) = {
        let locked_tunn_senders = tunn_senders.lock().unwrap();
        let concurrency = locked_tunn_senders.len() as u32;
        let tunn_id = seq % concurrency;
        let tunn_sender = locked_tunn_senders.get(&tunn_id).unwrap().clone();
        (tunn_id, tunn_sender)
    };
    if is_first_data {
        let frame = structures::FrameBindRequest {
            connection_id: connection_id,
            seq: 0,
            dest_host: "ya.ru".to_string(),
            dest_port: 80,
        };
        println!("bind send[{:?}]: {:?}", tunn_id, frame);
        tunn_sender.send(frame.to_bytes_with_header()).await?;
    }

    let frame = structures::FrameData {
        connection_id: connection_id,
        seq: seq,
        length: len as u32,
        data: data[..len].to_vec(),
    };
    println!("data send[{:?}]: {:?}", tunn_id, frame);
    tunn_sender.send(frame.to_bytes_with_header()).await?;
    Ok(())
}

async fn process_tunnel_data(
    data: Vec<u8>,
    offset: usize,
    conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    last_seq: Arc<Mutex<HashMap<u32, u32>>>,
    future_data: Arc<Mutex<HashMap<u32, HashMap<u32, Vec<u8>>>>>,
    tunnel_id: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    // read data from tunnel and send to client

    let frame_type = structures::get_frame_type(&data[offset..]);
    match frame_type {
        structures::FrameType::Data => {
            let frame = structures::FrameData::from_bytes(&data[offset..]);
            println!("data recv[{:?}]: {:?}", tunnel_id,  frame);
            let connection_id = frame.connection_id;
            let seq = frame.seq;

            let is_some_send_before = last_seq.lock().unwrap().contains_key(&connection_id);
            let last_send_seq = {
                if is_some_send_before {
                    last_seq.lock().unwrap().get(&connection_id).unwrap().clone()
                } else {
                    0
                }
            };

            let mut should_send = false;
            if !is_some_send_before && seq == 0 {
                should_send = true;
            }
            else if is_some_send_before && seq == last_send_seq + 1 {
                should_send = true;
            }

            if should_send {
                let conn_tx = {
                    let conn_senders = conn_senders.lock().unwrap();
                    let conn_tx = conn_senders.get(&frame.connection_id).unwrap().clone();
                    conn_tx
                };
                conn_tx.send(frame.data).await?;
                last_seq.lock().unwrap().insert(connection_id, seq);
                let mut next_seq = seq + 1;
                loop {
                    let mut future_data = future_data.lock().unwrap().clone();
                    let future_data_conn = future_data
                        .entry(connection_id)
                        .or_insert(HashMap::new());
                    if future_data_conn.contains_key(&next_seq) {
                        let conn_tx = {
                            let conn_senders = conn_senders.lock().unwrap();
                            let conn_tx = conn_senders.get(&frame.connection_id).unwrap().clone();
                            conn_tx
                        };
                        let data = future_data_conn.get(&next_seq).unwrap().clone();
                        conn_tx.send(data).await?;
                        last_seq.lock().unwrap().insert(connection_id, next_seq);
                        future_data_conn.remove(&next_seq);
                        next_seq = next_seq + 1;
                        continue;
                    }
                    break;
                }
            }
            else {
                let mut future_data = future_data.lock().unwrap();
                let future_data_conn = future_data
                    .entry(connection_id)
                    .or_insert(HashMap::new());
                future_data_conn.insert(seq, frame.data);
            }
        }
        _ => {
            panic!("unknown frame type")
        }
    }
    Ok(())
}

async fn create_tunnel(
    hostname: String,
    port: u16,
    number: u16,
    conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client_id = rand::random::<u32>();

    // connection_id -> seq -> data
    let future_data: Arc<Mutex<HashMap<u32, HashMap<u32, Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // connection_id -> seq
    let last_seq: Arc<Mutex<HashMap<u32, u32>>> = Arc::new(Mutex::new(HashMap::new()));

    for i in 0..number {
        let tunnel = TcpStream::connect(format!("{}:{}", hostname, port)).await?;
        let (tunnel_socket_reader, tunnel_socket_writer) = tunnel.into_split();

        let (sender_tx, mut sender_rx) = mpsc::channel(100);
        tunn_senders
            .lock()
            .unwrap()
            .insert(i as u32, sender_tx);

            let frame = structures::FrameAuth {
                client_id: client_id,
                key: 1234,
            };
            println!("auth send[{:?}]: {:?}", i, frame);
            tunn_senders.lock().unwrap().get(&(i as u32)).unwrap().send(frame.to_bytes_with_header()).await?;

        let conn_senders = conn_senders.clone();
        let last_seq = last_seq.clone();
        let future_data = future_data.clone();

        tokio::spawn(async move {
            if let Err(err) = read_tunnel_loop(tunnel_socket_reader, conn_senders, last_seq, future_data, i).await {
                println!("read_tunnel_loop error: {}", err);
            }
        });

        tokio::spawn(async move {
            if let Err(err) = write_tunnel_loop(tunnel_socket_writer, &mut sender_rx, i).await {
                println!("write_tunnel_loop error: {}", err);
            }
        });
    }
    Ok(())
}

async fn read_tunnel_loop(
    mut tunnel_socket_reader: OwnedReadHalf,
    conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    last_seq: Arc<Mutex<HashMap<u32, u32>>>,
    future_data: Arc<Mutex<HashMap<u32, HashMap<u32, Vec<u8>>>>>,
    tunnel_id: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_tunnel_loop");
    let mut prefix = Vec::new();
    loop {
        let mut read_buf = [0; MAX_FRAME_SIZE];
        let mut total_length = tunnel_socket_reader.read(&mut read_buf).await?;
        if total_length == 0 {
            break;
        }
        let mut buf = Vec::new();
        let prefix_length = prefix.len();
        if prefix_length > 0 {
            buf.extend_from_slice(&prefix[..prefix_length]);
            buf.extend_from_slice(&read_buf[..total_length]);
            total_length = total_length + prefix_length;
            prefix.clear();
        }
        else {
            buf.extend_from_slice(&read_buf[..total_length]);
        }
        let mut offset = 0;
        loop {
            if total_length - offset < 4 {
                if offset < total_length {
                    prefix = buf[offset..total_length].to_vec();
                }
                break;
            }
            let frame_length = structures::get_frame_length(&buf[offset..total_length]);
            if offset + frame_length > total_length {
                if offset < total_length {
                    prefix = buf[offset..total_length].to_vec();
                }
                break;
            }
            process_tunnel_data(buf.clone(), offset, conn_senders.clone(), last_seq.clone(), future_data.clone(), tunnel_id).await?;
            offset = offset + frame_length;
            if total_length - offset < 4 {
                break;
            }
        }
    }
    println!("end read_tunnel_loop");
    Ok(())
}

async fn write_tunnel_loop(
    mut tunnel_socket_writer: OwnedWriteHalf,
    sender_rx: &mut mpsc::Receiver<Vec<u8>>,
    tunnel_id: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start write_tunnel_loop");
    while let Some(buf) = sender_rx.recv().await {
        tunnel_socket_writer.write_all(&buf).await?;
    }
    println!("end write_tunnel_loop");
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
    connection_id: u32,
    mut sender_rx: mpsc::Receiver<Vec<u8>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    tokio::spawn(async move {
        if let Err(err) = read_client_loop(socket_reader, connection_id, tunn_senders).await {
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
    connection_id: u32,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_client_loop");
    let mut buf = [0; MAX_FRAME_SIZE];
    let mut is_first_data = true;
    let mut seq = 1;

    loop {
        let len = socket_reader.read(&mut buf).await?;
        if len == 0 {
            break;
        }
        process_client_data(
            buf, len, connection_id, seq, tunn_senders.clone(), is_first_data,
        ).await?;
        seq = seq + 1;
        is_first_data = false;
    }
    println!("end read_client_loop");
    Ok(())
}

async fn write_client_loop(
    mut socket_writer: OwnedWriteHalf,
    sender_rx: &mut mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    println!("start write_client_loop");
    while let Some(res) = sender_rx.recv().await {
        socket_writer.write_all(&res).await?;
    }
    println!("end write_client_loop");
    Ok(())
}
