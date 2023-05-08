mod structures;

use clap::Parser;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};

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

    let remote_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let listener = TcpListener::bind((args.listen, args.port)).await.unwrap();
    while let Ok((client_socket, _)) = listener.accept().await {
        let (sender_tx, sender_rx) = mpsc::channel(100);
        // TODO: get somehow different ids
        tunn_senders.lock().unwrap().insert(0, sender_tx);

        let remote_senders = remote_senders.clone();
        let tunn_senders = tunn_senders.clone();
        tokio::spawn(async move {
            handle_client_connection(client_socket, sender_rx, remote_senders, tunn_senders)
                .await
                .unwrap();
        });
    }
    Ok(())
}

async fn process_client_data(
    data: [u8; 1024],
    offset: usize,
    remote_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // read from tunnel client and pass to remotes

    let frame_type = structures::get_frame_type(&data[offset..]);
    match frame_type {
        structures::FrameType::BindRequest => {
            let frame = structures::FrameBindRequest::from_bytes(&data[offset..]);
            println!("bind recv: {:?}", frame);
            create_remote_conn(
                frame.dest_host,
                frame.dest_port,
                remote_senders.clone(),
                tunn_senders.clone(),
            )
            .await?;
        }
        structures::FrameType::Data => {
            let frame = structures::FrameData::from_bytes(&data[offset..]);
            println!("data recv: {:?}", frame);
            let remote_tx = remote_senders
                .lock()
                .unwrap()
                .values()
                .next()
                .expect("no remote connection found")
                .clone();
            remote_tx.send(frame.data).await?;
        }
        _ => {
            panic!("unknown frame type")
        }
    }
    Ok(())
}

async fn process_remote_data(
    data: [u8; 1024],
    len: usize,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // read from remote and pass to tunnel client

    let frame = structures::FrameData {
        connection_id: 0,
        seq: 0,
        length: len as u32,
        data: data[..len].to_vec(),
    };
    println!("data send: {:?}", frame);

    let tunn_tx = tunn_senders
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    tunn_tx.send(frame.to_bytes_with_header().to_vec()).await?;
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
    mut sender_rx: Receiver<Vec<u8>>,
    remote_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    tokio::spawn(async move {
        if let Err(err) =
            read_client_loop(socket_reader, remote_senders.clone(), tunn_senders.clone()).await
        {
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
    remote_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_client_loop");
    let mut buf = [0; 1024];
    loop {
        let total_length = socket_reader.read(&mut buf).await?;
        if total_length == 0 {
            break;
        }
        let mut offset = 0;
        loop {
            if total_length - offset < 4 {
                panic!("too short frame")
            }
            let frame_length = structures::get_frame_length(&buf[offset..total_length]);
            if offset + frame_length > total_length {
                panic!("too short frame")
            }
            process_client_data(buf, offset, remote_senders.clone(), tunn_senders.clone()).await?;
            offset = offset + frame_length;
            if total_length - offset < 4 {
                break;
            }
        }
    }
    println!("end read_client_loop");
    Ok(())
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
    hostname: String,
    port: u16,
    remote_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let tunnel = TcpStream::connect(format!("{}:{}", hostname, port)).await?;
    let (remote_socket_reader, remote_socket_writer) = tunnel.into_split();

    let remote_id = rand::random::<u32>();

    let (sender_tx, mut sender_rx) = mpsc::channel(100);
    remote_senders.lock().unwrap().insert(remote_id, sender_tx);

    let tunn_senders = tunn_senders.clone();

    tokio::spawn(async move {
        if let Err(err) = read_remote_loop(remote_socket_reader, tunn_senders).await {
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
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_remote_loop");
    let mut buf = [0; 1024];
    loop {
        let len = remote_socket_reader.read(&mut buf).await?;
        if len == 0 {
            break;
        }
        process_remote_data(buf, len, tunn_senders.clone()).await?;
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
        remote_socket_writer.write_all(&buf).await?;
    }
    println!("end write_remote_loop");
    Ok(())
}
