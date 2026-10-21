// Property tests standing in for cargo-fuzz (unreliable on Windows MSVC):
// arbitrary input must never panic the validators.
use nyx_core::{check_torrent_bytes, parse_magnet, safe_join};
use proptest::prelude::*;
use std::path::Path;

proptest! {
    // Any string into parse_magnet returns Ok/Err without panicking.
    #[test]
    fn magnet_never_panics(s in any::<String>()) {
        let _ = parse_magnet(&s);
    }

    // Magnet-shaped junk exercises the deeper branches (hash decode, percent decode).
    #[test]
    fn magnet_shaped_never_panics(s in "magnet:\\?[a-z0-9=&%:+._-]{0,200}") {
        let _ = parse_magnet(&s);
    }

    // Any bytes into check_torrent_bytes.
    #[test]
    fn torrent_never_panics(b in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = check_torrent_bytes(&b);
    }

    // Bytes biased toward bencode structure, so nesting/length paths get hit.
    #[test]
    fn torrent_structural_never_panics(b in proptest::collection::vec(prop::sample::select(b"ldie:0123456789".to_vec()), 0..2048)) {
        let _ = check_torrent_bytes(&b);
    }

    // Any Ok from safe_join stays under base and has no parent-dir component.
    #[test]
    fn safe_join_ok_stays_under_base(s in any::<String>()) {
        let base = Path::new(r"C:\dl");
        if let Ok(p) = safe_join(base, &s) {
            prop_assert!(p.starts_with(base));
            prop_assert!(!p.components().any(|c| matches!(c, std::path::Component::ParentDir)));
        }
    }

    // Path-shaped alphabet (dots, both slashes, colons, spaces) hits the tricky cases.
    #[test]
    fn safe_join_pathy_stays_under_base(s in "[a-zA-Z0-9./\\\\: ]{0,80}") {
        let base = Path::new(r"C:\dl");
        if let Ok(p) = safe_join(base, &s) {
            prop_assert!(p.starts_with(base));
        }
    }
}
