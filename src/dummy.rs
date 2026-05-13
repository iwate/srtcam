pub fn black_dummy_yuyv(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; (width as usize) * (height as usize) * 2];

    // YUYV422 black: Y=0, U=128, V=128 for each pixel pair.
    for chunk in out.chunks_exact_mut(4) {
        chunk[0] = 0;
        chunk[1] = 128;
        chunk[2] = 0;
        chunk[3] = 128;
    }

    out
}
