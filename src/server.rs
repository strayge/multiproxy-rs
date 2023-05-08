mod structures;

use clap::Parser;
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

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
        tokio::spawn(async move {
            handle_client_connection(client_socket).await.unwrap();
        });
    }
    Ok(())
}

async fn process_client_data(
    data: [u8; 1024],
    len: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: make func build frames and to send to free tunnel
    println!("client in: {:?}", String::from_utf8(data[..len].to_vec()));
    // let tunn_tx = tunn_senders.lock().unwrap().get(&0).unwrap().clone();
    // tunn_tx.send(data[..len].to_vec()).await?;
    Ok(())
}

async fn handle_client_connection(
    client_socket: TcpStream,
) -> Result<(), Box<dyn std::error::Error>> {
    let (socket_reader, socket_writer) = client_socket.into_split();

    tokio::spawn(async move {
        if let Err(err) = read_client_loop(socket_reader).await {
            println!("read_client_loop error: {}", err);
        }
    });

    tokio::spawn(async move {
        if let Err(err) = write_client_loop(socket_writer).await {
            println!("write_client_loop error: {}", err);
        }
    });

    Ok(())
}

async fn read_client_loop(
    mut socket_reader: OwnedReadHalf,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("start read_client_loop");
    let mut buf = [0; 1024];
    loop {
        let len = socket_reader.read(&mut buf).await?;
        println!("read from client");
        if len == 0 {
            break;
        }
        process_client_data(buf, len).await?;
    }
    println!("end read_client_loop");
    Ok(())
}

async fn write_client_loop(mut socket_writer: OwnedWriteHalf) -> io::Result<()> {
    println!("start write_client_loop");
    // while let Some(res) = sender_rx.recv().await {
    //     println!("write to client: {:?}", String::from_utf8(res.clone()));
    //     socket_writer.write_all(&res).await?;
    //     println!("write to client done")
    // }
    println!("end write_client_loop");
    Ok(())
}
