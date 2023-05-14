use std::net::{IpAddr, Ipv4Addr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub async fn socks5_preamble(
    mut socket_reader: OwnedReadHalf,
    mut socket_writer: OwnedWriteHalf,
) -> Result<(OwnedReadHalf, OwnedWriteHalf, String, u16), Box<dyn std::error::Error>> {
    let mut init_msg = [0; 2];
    socket_reader.read_exact(&mut init_msg).await?;
    if init_msg[0] != 5 {
        return Err(format!("invalid version[1]: {:?}", init_msg).into());
    }
    let methods_length = init_msg[1] as usize;
    let mut methods = vec![0; methods_length];
    socket_reader.read_exact(&mut methods).await?;

    let init_reply = vec![5, 0];
    socket_writer.write_all(&init_reply).await?;

    let mut bind_msg_start = [0; 4];
    socket_reader.read_exact(&mut bind_msg_start).await?;
    if bind_msg_start[0] != 5 {
        return Err(format!("invalid version[2]: {:?}", bind_msg_start).into());
    }
    if bind_msg_start[1] != 1 {
        let bind_response_failed = vec![5, 7, 0, bind_msg_start[3], 0, 0, 0, 0, 0, 0];
        socket_writer.write_all(&bind_response_failed).await?;
        return Err("unsuppported command".into());
    }
    let hostname;
    let port;
    if bind_msg_start[3] == 3 {
        let mut bind_hostname_len = [0; 1];
        socket_reader.read_exact(&mut bind_hostname_len).await?;
        let mut bind_hostname = vec![0; bind_hostname_len[0] as usize];
        socket_reader.read_exact(&mut bind_hostname).await?;
        let mut bind_port = [0; 2];
        socket_reader.read_exact(&mut bind_port).await?;
        hostname = String::from_utf8(bind_hostname)?;
        port = u16::from_be_bytes(bind_port);
    } else if bind_msg_start[3] == 1 {
        let mut bind_ip = [0; 4];
        socket_reader.read_exact(&mut bind_ip).await?;
        let mut bind_port = [0; 2];
        socket_reader.read_exact(&mut bind_port).await?;
        let ip = IpAddr::V4(Ipv4Addr::from(bind_ip));
        hostname = ip.to_string();
        port = u16::from_be_bytes(bind_port);
    } else {
        return Err("invalid address type".into());
    }
    let bind_response = vec![5, 0, 0, bind_msg_start[3], 0, 0, 0, 0, 0, 0];
    socket_writer.write_all(&bind_response).await?;
    Ok((socket_reader, socket_writer, hostname, port))
}
