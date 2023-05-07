use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Sender};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    let connections: Arc<Mutex<HashMap<u32, Sender<_>>>> = Arc::new(Mutex::new(HashMap::new()));

    while let Ok((client_socket, _)) = listener.accept().await {
        let (tx, rx) = mpsc::channel(100);
        let id = rand::random::<u32>();
        connections.lock().unwrap().insert(id, tx);

        let tx = connections.lock().unwrap().get(&id).unwrap().clone();
        tx.send(vec![31, 32, 33]).await.unwrap();
        println!("sent to tx");

        tokio::spawn(async move {
            handle_client_connection(client_socket, rx).await.unwrap();
        });
    }
    Ok(())
}

// TcpStream incapsulated in this function
async fn handle_client_connection(
    client_socket: TcpStream,
    mut rx: mpsc::Receiver<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, writer) = client_socket.into_split();
    let reader = Arc::new(tokio::sync::Mutex::new(reader));
    let writer = Arc::new(tokio::sync::Mutex::new(writer));

    async fn read_socket_loop(reader: Arc<tokio::sync::Mutex<OwnedReadHalf>>) {
        let mut buf = [0; 1024];
        let mut reader = reader.lock().await;
        loop {
            let len = reader.read(&mut buf).await.unwrap();
            if len == 0 {
                break;
            }
        }
    }

    tokio::spawn(async move {
        read_socket_loop(reader).await;
    });

    async fn read_queue_loop(
        writer: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
        rx: &mut mpsc::Receiver<Vec<u8>>
) {
        let mut writer = writer.lock().await;
        while let Some(res) = rx.recv().await {
            writer.write_all(&res).await.unwrap();
        }
    }

    tokio::spawn(async move {
        read_queue_loop(writer, &mut rx).await;
    });

    Ok(())
}
