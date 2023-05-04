mod structures;
mod tunnel_client;

use std::sync::{Arc, Mutex};

use clap::Parser;
use tokio::net::{TcpListener};
use tunnel_client::TunnelClient;


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
async fn main() -> std::io::Result<()> {
    let args = Cli::parse();

    // initialize tunnel
    let global_tunnel = Arc::new(Mutex::new(
        TunnelClient::new(args.server_host, args.server_port, args.concurrency)
    ));

    // tunnel.clone().connectToServer().await;

    // listen for actual clients
    let listener = TcpListener::bind((args.listen, args.port)).await?;

    loop {
        let (socket, _) = listener.accept().await?;

        let tunnel_loop = global_tunnel.clone();

        tokio::spawn(async move {

            let mut tunnel = tunnel_loop.lock().unwrap();
            tunnel.new_user_connection(socket).await;

        });

    }
}
