use rusticle::Gif;

#[test]
fn test_decode_empty_bytes() {
    // Empty bytes should fail gracefully
    let result = Gif::from_bytes(&[]);
    assert!(result.is_err());
}

#[test]
fn test_decode_from_read() {
    // Test that from_read is available
    let data: &[u8] = &[];
    let result = Gif::from_read(data);
    assert!(result.is_err());
}
