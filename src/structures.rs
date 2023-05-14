pub const FRAME_MAGIC: u16 = 0x1234;

#[derive(Debug)]
pub enum FrameType {
    Auth,
    Bind,
    Data,
    Close,
}

impl FrameType {
    pub fn from_number(number: u16) -> Self {
        match number {
            n if n == FrameType::Auth as u16 => FrameType::Auth,
            n if n == FrameType::Bind as u16 => FrameType::Bind,
            n if n == FrameType::Data as u16 => FrameType::Data,
            n if n == FrameType::Close as u16 => FrameType::Close,
            _ => panic!("Unknown frame type"),
        }
    }
}

pub trait Frame {
    fn get_type() -> FrameType;
    fn to_bytes(&self) -> Vec<u8>;
    fn from_bytes(bytes: &[u8]) -> Self;
    fn to_bytes_with_header(&self) -> Vec<u8> {
        let frame_bytes = self.to_bytes();
        let length = frame_bytes.len();
        let mut bytes = vec![];
        bytes.extend_from_slice(&FRAME_MAGIC.to_be_bytes());
        bytes.extend_from_slice(&(Self::get_type() as u16).to_be_bytes());
        bytes.extend_from_slice(&(length as u16).to_be_bytes());
        bytes.extend_from_slice(&frame_bytes);
        bytes
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_string(bytes: &[u8], offset: usize, size: u16) -> String {
    String::from_utf8(bytes[offset..offset + size as usize].to_vec()).unwrap()
}

fn read_vec_u8(bytes: &[u8], offset: usize, size: u32) -> Vec<u8> {
    bytes[offset..offset + size as usize].to_vec()
}

#[derive(Debug)]
pub struct FrameBind {
    pub connection_id: u32,
    pub seq: u32,
    pub dest_host: String,
    pub dest_port: u16,
}

impl Frame for FrameBind {
    fn get_type() -> FrameType {
        FrameType::Bind
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.connection_id.to_be_bytes());
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&(self.dest_host.len() as u16).to_be_bytes());
        bytes.extend_from_slice(self.dest_host.as_bytes());
        bytes.extend_from_slice(&self.dest_port.to_be_bytes());
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let connection_id = read_u32(bytes, 0);
        let seq = read_u32(bytes, 4);
        let dest_host_len = read_u16(bytes, 8);
        let dest_host = read_string(bytes, 10, dest_host_len);
        let dest_port = read_u16(bytes, 10 + dest_host_len as usize);
        Self { connection_id, seq, dest_host, dest_port }
    }
}

#[derive(Debug)]
pub struct FrameData {
    pub connection_id: u32,
    pub seq: u32,
    pub length: u32,
    pub data: Vec<u8>,
}

impl Frame for FrameData {
    fn get_type() -> FrameType {
        FrameType::Data
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.connection_id.to_be_bytes());
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&self.length.to_be_bytes());
        bytes.extend_from_slice(&self.data);
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let connection_id = read_u32(bytes, 0);
        let seq = read_u32(bytes, 4);
        let length = read_u32(bytes, 8);
        let data = read_vec_u8(bytes, 12, length);
        Self { connection_id, seq, length, data }
    }
}

#[derive(Debug)]
pub struct FrameClose {
    pub connection_id: u32,
    pub seq: u32,
}

impl Frame for FrameClose {
    fn get_type() -> FrameType {
        FrameType::Close
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.connection_id.to_be_bytes());
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let connection_id = read_u32(bytes, 0);
        let seq = read_u32(bytes, 4);
        Self { connection_id, seq }
    }
}

#[derive(Debug)]
pub struct FrameAuth {
    pub client_id: u32,
    pub key: u32,
}

impl Frame for FrameAuth {
    fn get_type() -> FrameType {
        FrameType::Auth
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.client_id.to_be_bytes());
        bytes.extend_from_slice(&self.key.to_be_bytes());
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let client_id = read_u32(bytes, 0);
        let key = read_u32(bytes, 4);
        Self { client_id, key }
    }
}
