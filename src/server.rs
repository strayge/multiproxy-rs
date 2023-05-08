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
    len: usize,
    remote_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // read from tunnel client and pass to remotes
    println!("client in: {:?}", String::from_utf8(data[..len].to_vec()));

    // DEV: remove this and create remote connection from bind frame
    if data[..len].to_vec() == b"connect\n" {
        create_remote_conn(
            "ya.ru".to_string(),
            80,
            remote_senders.clone(),
            tunn_senders.clone(),
        )
        .await?;
        return Ok(());
    }

    let remote_tx = remote_senders
        .lock()
        .unwrap()
        .values()
        .next()
        .expect("no remote connection found")
        .clone();
    remote_tx.send(data[..len].to_vec()).await?;
    Ok(())
}

async fn process_remote_data(
    data: [u8; 1024],
    len: usize,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // read from remote and pass to tunnel client
    println!("remote in: {:?}", String::from_utf8(data[..len].to_vec()));
    let tunn_tx = tunn_senders
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    tunn_tx.send(data[..len].to_vec()).await?;
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
        let len = socket_reader.read(&mut buf).await?;
        println!("read from client");
        if len == 0 {
            break;
        }
        process_client_data(buf, len, remote_senders.clone(), tunn_senders.clone()).await?;
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
        println!("write to client: {:?}", String::from_utf8(res.clone()));
        socket_writer.write_all(&res).await?;
        println!("write to client done")
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
        println!("read from remote");
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
        println!("write to tunnel: {:?}", String::from_utf8(buf.clone()));
        remote_socket_writer.write_all(&buf).await?;
    }
    println!("end write_remote_loop");
    Ok(())
}
