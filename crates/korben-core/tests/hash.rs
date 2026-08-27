//! SHA-256 against the published test vectors.

use korben_core::hash::{checksum, hex, Sha256};

fn digest(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex(&hasher.finish())
}

#[test]
fn matches_the_published_vectors() {
    assert_eq!(digest(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(digest(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(
        digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn handles_input_spanning_many_blocks() {
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        digest(&million),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn incremental_updates_match_a_single_pass() {
    let mut split = Sha256::new();
    split.update(b"korben ");
    split.update(b"is ");
    split.update(b"content addressed");
    assert_eq!(hex(&split.finish()), digest(b"korben is content addressed"));
}

#[test]
fn checksums_are_prefixed() {
    assert!(checksum(b"abc").starts_with("sha256:"));
    assert_eq!(checksum(b"abc").len(), "sha256:".len() + 64);
}
