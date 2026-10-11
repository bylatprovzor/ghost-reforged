//! Windows x64 leaf function: bool send(this, bytes, length).
//! Forward all traffic except exactly framed W3GS PlayerExtra/profile messages.
pub(super) fn machine_code(original: usize, counter: usize) -> Vec<u8> {
    let mut code = Vec::new();
    let mut jumps = Vec::new();
    for (check, branch) in [
        (&[0x41, 0x83, 0xf8, 0x09][..], 0x72),       // length >= 9
        (&[0x48, 0x85, 0xd2][..], 0x74),             // buffer != null
        (&[0x66, 0x81, 0x3a, 0xf7, 0x59][..], 0x75), // F7 59
        (&[0x80, 0x7a, 0x04, 0x03][..], 0x75),       // profile subtype
        (&[0x0f, 0xb7, 0x42, 0x02, 0x44, 0x39, 0xc0][..], 0x75), // frame length
        (
            &[0x8b, 0x42, 0x05, 0x83, 0xc0, 0x09, 0x44, 0x39, 0xc0][..],
            0x75,
        ), // payload length
    ] {
        code.extend_from_slice(check);
        code.extend_from_slice(&[branch, 0]);
        jumps.push(code.len() - 1);
    }
    code.extend_from_slice(&[0x48, 0xb8]); // mov rax, counter
    code.extend_from_slice(&(counter as u64).to_le_bytes());
    code.extend_from_slice(&[0xf0, 0x48, 0xff, 0x00]); // lock inc qword [rax]
    code.extend_from_slice(&[0xb8, 1, 0, 0, 0, 0xc3]); // successful return
    let forward = code.len();
    code.extend_from_slice(&[0x48, 0xb8]);
    code.extend_from_slice(&(original as u64).to_le_bytes());
    code.extend_from_slice(&[0xff, 0xe0]); // tail-call, preserve args and nonvolatile registers
    for position in jumps {
        code[position] = u8::try_from(forward - position - 1).expect("short branch");
    }
    code
}
