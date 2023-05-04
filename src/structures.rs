pub const FRAME_MAGIC: u16 = 0x1234;

pub enum FrameType {
    BindRequest(u8),
    BindResponse(u8),
    Data(u8),
    CloseRequest(u8),
    CloseResponse(u8),
}

pub enum Frame {
    BindRequest {
        connection_id: u32,
        dest_host: String,
        dest_port: u16,
    },
    BindResponse {
        connection_id: u32,
    },
    Data {
        connection_id: u32,
        seq: u32,
        length: u32,
        data: Vec<u8>,
    },
    CloseRequest {
        connection_id: u32,
    },
    CloseResponse {
        connection_id: u32,
    },
}

pub struct FullFrame {
    pub magic: u16,
    pub frame_type: FrameType,
    pub frame: Frame,
}
