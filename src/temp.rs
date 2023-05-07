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

    async fn read_socket_loop(mut reader: OwnedReadHalf) {
        println!("start read loop");
        let mut buf = [0; 1024];
        loop {
            let len = reader.read(&mut buf).await.unwrap();
            println!("read from client");
            if len == 0 {
                break;
            }
        }
        println!("end read loop");
    }

    async fn read_queue_loop(
        mut writer: OwnedWriteHalf,
        rx: &mut mpsc::Receiver<Vec<u8>>
    ) {
        println!("start queue loop");
        while let Some(res) = rx.recv().await {
            println!("write to client");
            writer.write_all(&res).await.unwrap();
            println!("write to client done")
        }
        println!("end queue loop");
    }

    tokio::spawn(async move {
        read_socket_loop(reader).await;
    });

    tokio::spawn(async move {
        read_queue_loop(writer, &mut rx).await;
    });

    Ok(())
}
