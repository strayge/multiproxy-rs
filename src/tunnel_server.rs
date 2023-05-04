use tokio::net::TcpStream;


pub struct TunnelServer {
    field: u16,
}

impl TunnelServer {
    pub fn new() -> Self {
        Self {
            field: 0,
        }
    }

    pub async fn new_client_connection(&mut self, socket: TcpStream) {
        println!("TunnelServer::new_client_connection {:?}", socket);
    }

}
