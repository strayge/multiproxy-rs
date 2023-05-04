use tokio::net::TcpStream;


pub struct TunnelClient {
    server_host: String,
    server_port: u16,
    concurrency: u16,
    connections: Vec<TcpStream>,
}

impl TunnelClient {
    pub fn new(server_host: String, server_port: u16, concurrency: u16) -> Self {
        Self {
            server_host,
            server_port,
            concurrency,
            connections: Vec::new(),
        }
    }

    pub async fn connectToServer(&mut self) {
        println!("TunnelClient::connectToServer");
        for _ in 0..self.concurrency {
            let socket = TcpStream::connect((self.server_host.as_str(), self.server_port)).await.unwrap();
            println!("connected to server: {:?}", socket);
            self.connections.push(socket);
        }
    }

    pub async fn new_user_connection(&mut self, socket: TcpStream) {
        println!("TunnelClient::new_user_connection {:?}", socket);
    }

}

// // Handle full lifecycle of client connection
// async fn clientConnectHandler(mut tunnel: TunnelClient, socket: TcpStream) {
//     // hardcoded for now
//     let host = "ya.ru";
//     let port = 80;

//     // make queue for current connection
//     let (client2tunnel_tx, mut client2tunnel_rx) = mpsc::channel(tunnel.concurrency as usize * 2);
//     let (tunnel2client_tx, mut tunnel2client_rx) = mpsc::channel(tunnel.concurrency as usize * 2);

//     // generate random u32
//     let connection_id = rand::thread_rng().gen::<u32>();

//     tunnel.add_connection(connection_id, client2tunnel_rx, tunnel2client_tx);
//     tunnel.connect(connection_id, host, port).await;

//     tokio::spawn(async move {
//         // make connect to provided host:port
//         let mut frame = FrameBind {
//             magic: 0x12345678,
//             dest_host: host.to_string(),
//             dest_port: port,
//         };
//         queue_tx.send(frame).await;
//     });

//     tunnel.bind(frame).await;

// }
