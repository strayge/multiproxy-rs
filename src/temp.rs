use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Sender};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // send data to actual clients
    let conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // receive data from tunnel server
    let tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    for i in 0..1 {
        let tunnel = TcpStream::connect("ya.ru:80").await?;
        let (tunnel_socket_reader, tunnel_socket_writer) = tunnel.into_split();

        let (sender_tx, mut sender_rx) = mpsc::channel(100);
        tunn_senders.lock().unwrap().insert(i, sender_tx);

        let conn_senders = conn_senders.clone();

        tokio::spawn(async move {
            if let Err(err) = read_tunnel_loop(tunnel_socket_reader, conn_senders).await {
                println!("read_tunnel_loop error: {}", err);
            }
        });

        tokio::spawn(async move {
            if let Err(err) = write_tunnel_loop(tunnel_socket_writer, &mut sender_rx).await {
                println!("write_tunnel_loop error: {}", err);
            }
        });
    }

    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    while let Ok((client_socket, _)) = listener.accept().await {
        let id = rand::random::<u32>();

        let (sender_tx, sender_rx) = mpsc::channel(100);
        conn_senders.lock().unwrap().insert(id, sender_tx);

        let tunn_senders = tunn_senders.clone();

        tokio::spawn(async move {
            handle_client_connection(
                client_socket,
                id,
                sender_rx,
                tunn_senders,
            ).await.unwrap();
        });

    }

    Ok(())
}

async fn process_client_data(
    data: [u8; 1024],
    len: usize,
    id: u32,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: make func build frames and to send to free tunnel
    println!("client in: {:?}", String::from_utf8(data[..len].to_vec()));
    let tunn_tx = tunn_senders.lock().unwrap().get(&0).unwrap().clone();
    tunn_tx.send(data[..len].to_vec()).await?;
    Ok(())
}

async fn process_tunnel_data(
    data: [u8; 1024],
    len: usize,
    conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: make func to parse/detect conn/rearrange etc...
    println!("tunnel in: {:?}", String::from_utf8(data[..len].to_vec()));
    let conn_tx = conn_senders.lock().unwrap().values().next().unwrap().clone();
    conn_tx.send(data[..len].to_vec()).await?;
    Ok(())
}

async fn read_tunnel_loop(
    mut tunnel_socket_reader: OwnedReadHalf,
    conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_tunnel_loop");
    let mut buf = [0; 1024];
    loop {
        let len = tunnel_socket_reader.read(&mut buf).await?;
        println!("read from tunnel");
        if len == 0 {
            break;
        }
        process_tunnel_data(buf, len, conn_senders.clone()).await?;
    }
    println!("end read_tunnel_loop");
    Ok(())
}

async fn write_tunnel_loop(
    mut tunnel_socket_writer: OwnedWriteHalf,
    sender_rx: &mut mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start write_tunnel_loop");
    while let Some(buf) = sender_rx.recv().await {
        println!("write to tunnel: {:?}", String::from_utf8(buf.clone()));
        tunnel_socket_writer.write_all(&buf).await?;
    }
    println!("end write_tunnel_loop");
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
    id: u32,
    mut sender_rx: mpsc::Receiver<Vec<u8>>,
    tunn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    tokio::spawn(async move {
        if let Err(err) = read_client_loop(socket_reader, id, tunn_senders).await {
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
    id: u32,
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
        process_client_data(buf, len, id, tunn_senders.clone()).await?;
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
        println!("write to client: {:?}", String::from_utf8(res.clone()));
        socket_writer.write_all(&res).await?;
        println!("write to client done")
    }
    println!("end write_client_loop");
    Ok(())
}
