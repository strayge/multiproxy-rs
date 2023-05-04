mod structures;
mod tunnel_server;

use tokio::net::{TcpListener};
use clap::Parser;
use tunnel_server::TunnelServer;


#[derive(Parser)]
#[command(long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "127.0.0.1")]
    listen: String,

    #[arg(short, long, default_value = "5050")]
    port: u16,
}


#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let listener = TcpListener::bind((args.listen, args.port)).await.unwrap();

    let tunnel_server = TunnelServer::new();

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            tunnel_server.new_client_connection(socket).await;
        });
    }
}
