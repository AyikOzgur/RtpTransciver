use std::{net::UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};
use std::thread;
use std::time::Duration;
const MAX_RTP_BUF_SIZE: usize = 1400;
const RTP_HEADER_SIZE: usize = 12;


struct RtpHeader {
    byte1: u8,
    byte2: u8,
    seq: u16,
    ts: u32,
    ssrc: u32
}

impl RtpHeader {
    fn copy_into_array(&self) -> [u8; RTP_HEADER_SIZE] {
        let mut array: [u8; RTP_HEADER_SIZE] = [0u8; RTP_HEADER_SIZE];
        array[0] = self.byte1;
        array[1] = self.byte2;
        array[2..4].copy_from_slice(&self.seq.to_be_bytes());
        array[4..8].copy_from_slice(&self.ts.to_be_bytes());
        array[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        array
    }
}

pub struct H264RtpPusher {
    socket: UdpSocket,
    destination_address: String,

    rtp_buffer: [u8; 2048],
    rtp_buffer_size: usize,
    rtp_ts: u32,
    rtp_type: u8,
    rtp_seq: u16,
    rtp_is_last: bool
}

impl H264RtpPusher {
    pub fn new(destination: String) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:1234").unwrap();
        Self {
            socket: socket,
            destination_address: destination,
            rtp_buffer: [0u8; 2048],
            rtp_buffer_size : 0,
            rtp_ts: 0,
            rtp_type: 0,
            rtp_seq: 0,
            rtp_is_last: false
        }
    }

    pub fn send_frame(&mut self, frame_buffer: &[u8]) {
        let mut remaining = frame_buffer;
        loop {
            match get_nal(remaining) {
                Some((nal_type, nal_buf, is_last)) => {
                    self.handle_nal(nal_buf, nal_type);
                    remaining = &remaining[nal_buf.len()..];
                }
                None => { break; }
            }
        }
    }

    fn handle_nal(&mut self, nal_buf: &[u8], nal_type: H264NalType) {
        self.rtp_ts = self.get_timestamp();

        // Nal does not need FU-A fragmentation.
        if nal_buf.len() + RTP_HEADER_SIZE <= MAX_RTP_BUF_SIZE {
            self.rtp_buffer_size = nal_buf.len() + RTP_HEADER_SIZE;
            self.rtp_is_last = true;

            let offset = RTP_HEADER_SIZE; // start copying after the RTP header
            let len = nal_buf.len();      // number of bytes to copy

            // Slice the destination exactly the same length as the source
            self.rtp_buffer[offset..offset + len].copy_from_slice(nal_buf);

            // Send over UDP.
            self.send_rtp_over_udp();
        } else {
            const FU_A_SIZE: usize = 2;
            let mut FU_A: [u8; FU_A_SIZE] = [0u8; FU_A_SIZE];
            // Set FU indicator type to FU-A
            FU_A[0] |= 28;
            // Set F bit to 0
            FU_A[0] &= !(1 << 7);

            let nri_value = match nal_type {
                H264NalType::Sps |
                H264NalType::Pps | 
                H264NalType::Idr | 
                H264NalType::NonIdr => {
                    3
                } 
                _ => {
                    0
                }
            };

            // Set the NRI bits to the desired value
            FU_A[0] |= nri_value << 5;
            // Set FU header type with inputFrameType
            FU_A[1] |= nal_type as u8;
            // Set reserved bit (bit 5) to 0
            FU_A[1] &= !(1 << 5);
            // Set start bit to 1
            FU_A[1] |= 1 << 7;
            // Set end bit to 0
            FU_A[1] &= !(1 << 6);

            // Skip the original header.
            let mut remaining_nal = &nal_buf[1..];

            while remaining_nal.len() + FU_A_SIZE >= MAX_RTP_BUF_SIZE {
                self.rtp_buffer_size = RTP_HEADER_SIZE + MAX_RTP_BUF_SIZE;
                self.rtp_is_last = false;

                self.rtp_buffer[RTP_HEADER_SIZE..RTP_HEADER_SIZE + FU_A.len()].copy_from_slice(&FU_A);

                let packet_size = MAX_RTP_BUF_SIZE - FU_A_SIZE;
                self.rtp_buffer[RTP_HEADER_SIZE + FU_A_SIZE..RTP_HEADER_SIZE + FU_A_SIZE + packet_size].copy_from_slice(&remaining_nal[..packet_size]);

                // Send rtp over udp
                self.send_rtp_over_udp();

                remaining_nal = &remaining_nal[packet_size..];

                // Set start bit to 0
                FU_A[1] &= !(1 << 7);
            }

            // Last packet.
            self.rtp_buffer_size = RTP_HEADER_SIZE + FU_A_SIZE + remaining_nal.len();
            self.rtp_is_last = true;
            
            // Set end bit to 1. This is the last packet in the frame.
            FU_A[1] |= 1 << 6;
            
            self.rtp_buffer[RTP_HEADER_SIZE..RTP_HEADER_SIZE + FU_A.len()].copy_from_slice(&FU_A);
            self.rtp_buffer[RTP_HEADER_SIZE + FU_A_SIZE..RTP_HEADER_SIZE + FU_A_SIZE + remaining_nal.len()].copy_from_slice(&remaining_nal); // this is the line

            // Send Rtp over udp.
            self.send_rtp_over_udp();

        }
    }

    fn send_rtp_over_udp(&mut self) {
        let mut rtp_header = RtpHeader {
            byte1: 0,
            byte2: 0,
            seq: 0,
            ssrc: 0,
            ts: 0
        };

        if self.rtp_is_last {
            rtp_header.byte2 |= 1 << 7;
        } else {
            rtp_header.byte2 &= !(1 << 7);
        }

        rtp_header.byte2 |= 96;
        rtp_header.byte1 |= 2 << 6;

        rtp_header.seq = self.rtp_seq;
        rtp_header.ts = self.rtp_ts;
        rtp_header.ssrc = 12345u32;

        self.rtp_seq += 1;

        let rtp_header_buffer = rtp_header.copy_into_array();

        self.rtp_buffer[..RTP_HEADER_SIZE].copy_from_slice(&rtp_header_buffer);

        self.socket.send_to(&self.rtp_buffer[..self.rtp_buffer_size], self.destination_address.clone());

        // This delay should be calculated based on network bandwidth in a real case usage.
        //thread::sleep(Duration::from_millis(10)); 

    }

    fn get_timestamp(&self) -> u32 {
        // Get current time since epoch in microseconds
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        // Same formula: (micros + 500) / 1000 * 90
        let ts90k = ((micros + 500) / 1000) * 90;

        ts90k as u32
    }
}

#[repr(u8)]
#[derive(PartialEq)]
enum H264NalType {
    UnKnown = 0,
    NonIdr = 1,
    Idr = 5,
    Sei = 6,
    Sps = 7,
    Pps = 8,
    Aud = 9,
    EndOfSeq = 10,
    EndOfStream = 11,
    Filler = 12,
}

fn get_nal(input_buffer: &[u8]) -> Option<(H264NalType, &[u8], bool)> {
    const MAX_START_CODE_LENGTH: usize = 4;

    let mut is_start_found: bool = false;
    let mut is_end_found: bool = false;
    let mut start_code: usize = 0;
    let mut nal_type: H264NalType = H264NalType::UnKnown;
    let mut nal_start_index: usize = 0;
    let mut nal_end_index: usize = 0;

    if input_buffer.len() <= MAX_START_CODE_LENGTH {
        return None;
    }

    // Find the first nal unit.
    for index in 0..input_buffer.len() - MAX_START_CODE_LENGTH {
        if input_buffer[index] == 0 && input_buffer[index + 1] == 0 && input_buffer[index + 2] == 1
        {
            start_code = 3;
        } else if input_buffer[index] == 0
            && input_buffer[index + 1] == 0
            && input_buffer[index + 2] == 0
            && input_buffer[index + 3] == 1
        {
            start_code = 4;
        } else {
            continue;
        }

        let possible_nal_start = input_buffer[index + start_code];
        let possible_nal_type = possible_nal_start & 0x1F;
        let possible_nal_type_enum: H264NalType = match possible_nal_type {
            1 => H264NalType::NonIdr,
            5 => H264NalType::Idr,
            6 => H264NalType::Sei,
            7 => H264NalType::Sps,
            8 => H264NalType::Pps,
            9 => H264NalType::Aud,
            10 => H264NalType::EndOfSeq,
            11 => H264NalType::EndOfStream,
            12 => H264NalType::Filler,
            _ => H264NalType::UnKnown,
        };

        // Check if we found a valid nal.
        if possible_nal_type_enum != H264NalType::UnKnown {
            nal_type = possible_nal_type_enum;
            nal_start_index = index + start_code;
            is_start_found = true;
            break;
        }
    }

    // If there is no start, no need to look for next one.
    if !is_start_found {
        return None;
    }

    // Find second Nal unit.
    for index in nal_start_index + start_code..input_buffer.len() - MAX_START_CODE_LENGTH {
        if input_buffer[index] == 0 && input_buffer[index + 1] == 0 && input_buffer[index + 2] == 1
        {
            start_code = 3;
        } else if input_buffer[index] == 0
            && input_buffer[index + 1] == 0
            && input_buffer[index + 2] == 0
            && input_buffer[index + 3] == 1
        {
            start_code = 4;
        } else {
            continue;
        }

        let possible_nal_start = input_buffer[index + start_code];
        let possible_nal_type = possible_nal_start & 0x1F;
        let possible_nal_type_enum: H264NalType = match possible_nal_type {
            1 => H264NalType::NonIdr,
            5 => H264NalType::Idr,
            6 => H264NalType::Sei,
            7 => H264NalType::Sps,
            8 => H264NalType::Pps,
            9 => H264NalType::Aud,
            10 => H264NalType::EndOfSeq,
            11 => H264NalType::EndOfStream,
            12 => H264NalType::Filler,
            _ => H264NalType::UnKnown,
        };

        // Check if we found a valid nal.
        if possible_nal_type_enum != H264NalType::UnKnown {
            is_end_found = true;
            nal_end_index = index;
            break;
        }
    }

    if is_start_found && is_end_found {
        return Some((nal_type, &input_buffer[nal_start_index..nal_end_index], false));
    } else if is_start_found && !is_end_found {
        return Some((nal_type, &input_buffer[nal_start_index..], true));
    }

    return None;
}