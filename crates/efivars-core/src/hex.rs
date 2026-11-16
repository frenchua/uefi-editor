//! Small hex helpers shared by decoders and the UI.

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex of at most `max` bytes, with an ellipsis when truncated.
pub fn preview(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        to_hex(bytes)
    } else {
        format!("{}… ({} bytes)", to_hex(&bytes[..max]), bytes.len())
    }
}

/// Classic 16-bytes-per-line dump: offset, hex columns and ASCII gutter.
pub fn dump_lines(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(16)
        .enumerate()
        .map(|(i, chunk)| {
            let mut line = format!("{:04x}  ", i * 16);
            for j in 0..16 {
                match chunk.get(j) {
                    Some(b) => line.push_str(&format!("{b:02x} ")),
                    None => line.push_str("   "),
                }
            }
            line.push(' ');
            line.extend(chunk.iter().map(|&b| printable(b)));
            line
        })
        .collect()
}

pub fn printable(b: u8) -> char {
    if (0x20..0x7f).contains(&b) { b as char } else { '.' }
}
