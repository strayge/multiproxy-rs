use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Sender, Receiver};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    let conn_senders: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let conn_receivers: Arc<Mutex<HashMap<u32, Arc<Mutex<Receiver<Vec<u8>>>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    while let Ok((client_socket, _)) = listener.accept().await {
        let id = rand::random::<u32>();

        let (sender_tx, sender_rx) = mpsc::channel(100);
        conn_senders.lock().unwrap().insert(id, sender_tx);

        let (receiver_tx, receiver_rx) = mpsc::channel(100);
        conn_receivers.lock().unwrap().insert(id, Arc::new(Mutex::new(receiver_rx)));

        tokio::spawn(async move {
            handle_client_connection(client_socket, sender_rx, receiver_tx).await.unwrap();
        });

        // DEV: temporal glue for connecting sender and receiver
        let receiver_rx = {
            let receivers = conn_receivers.lock();
            receivers.unwrap().get(&id).unwrap().clone()
        };
        let sender_tx = {
            let senders = conn_senders.lock();
            senders.unwrap().get(&id).unwrap().clone()
        };
        while let Some(res) = receiver_rx.lock().unwrap().recv().await {
            sender_tx.send(res).await.unwrap();
        }

    }

    Ok(())
}


// TcpStream incapsulated in this function
async fn handle_client_connection(
    client_socket: TcpStream,
    mut sender_rx: mpsc::Receiver<Vec<u8>>,
    mut receiver_tx: mpsc::Sender<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    async fn read_socket_loop(
        mut socket_reader: OwnedReadHalf,
        receiver_tx: &mut mpsc::Sender<Vec<u8>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("start read loop");
        let mut buf = [0; 1024];
        loop {
            let len = socket_reader.read(&mut buf).await?;
            println!("read from client");
            if len == 0 {
                break;
            }
            receiver_tx.send(buf[..len].to_vec()).await?;
        }
        println!("end read loop");
        Ok(())
    }

    async fn read_queue_loop(
        mut socket_writer: OwnedWriteHalf,
        sender_rx: &mut mpsc::Receiver<Vec<u8>>,
    ) -> io::Result<()> {
        println!("start queue loop");
        while let Some(res) = sender_rx.recv().await {
            println!("write to client");
            socket_writer.write_all(&res).await?;
            println!("write to client done")
        }
        println!("end queue loop");
        Ok(())
    }

    tokio::spawn(async move {
        if let Err(err) = read_socket_loop(socket_reader, &mut receiver_tx).await {
            println!("read_socket_loop error: {}", err);
        }
    });

    tokio::spawn(async move {
        if let Err(err) = read_queue_loop(socket_writer, &mut sender_rx).await {
            println!("read_queue_loop error: {}", err);
        }
    });

    Ok(())
}
