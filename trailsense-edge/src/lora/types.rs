pub struct gateway_send_packet {
    v: u16, // Version Number
    i: u32, // Node Id
    b: u16, // Boot Id
    s: u32, // Sequence Id
    m: str, // Message (ACK, ERR, SLEEP)
}

pub const LORA_FREQUENCY_IN_HZ: u32 = 868_000_000;
