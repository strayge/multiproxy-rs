pub const FRAME_MAGIC: u16 = 0x1234;

pub enum FrameType {
    BindRequest = 0,
    BindResponse = 1,
    Data = 2,
    CloseRequest = 3,
    CloseResponse = 4,
}

pub fn get_frame_length(bytes: &[u8]) -> usize {
    let mut offset = 0;
    let magic = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
    offset += 2;
    assert_eq!(magic, FRAME_MAGIC);
    let length = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
    offset += 2;
    let total_length = length + offset as u16;
    total_length as usize
}

pub fn get_frame_type(bytes: &[u8]) -> FrameType {
    let offset = 4;
    let frame_type = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
    match frame_type {
        0 => FrameType::BindRequest,
        1 => FrameType::BindResponse,
        2 => FrameType::Data,
        3 => FrameType::CloseRequest,
        4 => FrameType::CloseResponse,
        _ => panic!("Unknown frame type"),
    }
}

#[derive(Debug)]
pub struct FrameBindRequest {
    pub connection_id: u32,
    pub dest_host: String,
    pub dest_port: u16,
}

impl FrameBindRequest {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.connection_id.to_be_bytes());
        bytes.extend_from_slice(&(self.dest_host.len() as u16).to_be_bytes());
        bytes.extend_from_slice(self.dest_host.as_bytes());
        bytes.extend_from_slice(&self.dest_port.to_be_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut offset = 6;
        let connection_id = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let dest_host_len = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
        offset += 2;
        let dest_host =
            String::from_utf8(bytes[offset..offset + dest_host_len as usize].to_vec()).unwrap();
        offset += dest_host_len as usize;
        let dest_port = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
        Self {
            connection_id,
            dest_host,
            dest_port,
        }
    }

    pub fn to_bytes_with_header(&self) -> Vec<u8> {
        let frame_bytes = self.to_bytes();
        let length = frame_bytes.len() + 2;
        let mut bytes = vec![];
        bytes.extend_from_slice(&FRAME_MAGIC.to_be_bytes());
        bytes.extend_from_slice(&(length as u16).to_be_bytes());
        bytes.extend_from_slice(&(FrameType::BindRequest as u16).to_be_bytes());
        bytes.extend_from_slice(&frame_bytes);
        bytes
    }
}

// pub struct FrameBindResponse {
//     connection_id: u32,
// }

#[derive(Debug)]
pub struct FrameData {
    pub connection_id: u32,
    pub seq: u32,
    pub length: u32,
    pub data: Vec<u8>,
}

impl FrameData {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bytes.extend_from_slice(&self.connection_id.to_be_bytes());
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&self.length.to_be_bytes());
        bytes.extend_from_slice(&self.data);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut offset = 6;
        let connection_id = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let seq = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let data = bytes[offset..offset + length as usize].to_vec();
        Self {
            connection_id,
            seq,
            length,
            data,
        }
    }

    pub fn to_bytes_with_header(&self) -> Vec<u8> {
        let frame_bytes = self.to_bytes();
        let length = frame_bytes.len() + 2;
        let mut bytes = vec![];
        bytes.extend_from_slice(&FRAME_MAGIC.to_be_bytes());
        bytes.extend_from_slice(&(length as u16).to_be_bytes());
        bytes.extend_from_slice(&(FrameType::Data as u16).to_be_bytes());
        bytes.extend_from_slice(&frame_bytes);
        bytes
    }
}

// pub struct FrameCloseRequest {
//     connection_id: u32,
// }

// pub struct FrameCloseResponse {
//     connection_id: u32,
// }
