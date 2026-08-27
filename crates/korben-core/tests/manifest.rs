//! Manifest parsing and bundle round-tripping.

// korben-6bc

use korben_core::bundle;
use korben_core::manifest::Manifest;

#[test]
fn parses_the_documented_manifest_shape() {
    let text = r#"
[package]
name = "hello-service"   # inline comment
version = "0.1.0"
edition = "2026"
description = "Example Korben service"
license = "MIT"

[dependencies]
http = "^0.1"
json = "^0.1"

[dev-dependencies]
testkit = "^0.1"

[build]
target = "native"
opt-level = 2
capabilities = ["fs", "net"]
"#;
    let manifest = Manifest::parse(text, None).expect("manifest should parse");
    assert_eq!(manifest.name, "hello-service");
    assert_eq!(manifest.version, "0.1.0");
    assert_eq!(manifest.edition, "2026");
    assert_eq!(manifest.description.as_deref(), Some("Example Korben service"));
    assert_eq!(manifest.dependencies.get("http").map(String::as_str), Some("^0.1"));
    assert_eq!(manifest.dev_dependencies.get("testkit").map(String::as_str), Some("^0.1"));
    assert_eq!(manifest.opt_level, 2);
    assert_eq!(manifest.build_capabilities, vec!["fs", "net"]);
}

#[test]
fn a_manifest_without_a_name_is_rejected() {
    assert!(Manifest::parse("[package]\nversion = \"1.0\"\n", None).is_err());
}

#[test]
fn rendered_manifests_round_trip() {
    let mut manifest = Manifest::default_for("demo");
    manifest.license = Some("MIT".to_string());
    manifest.dependencies.insert("http".to_string(), "^0.1".to_string());
    let reparsed = Manifest::parse(&manifest.render(), None).expect("round trip");
    assert_eq!(reparsed.name, "demo");
    assert_eq!(reparsed.license.as_deref(), Some("MIT"));
    assert_eq!(reparsed.dependencies.get("http").map(String::as_str), Some("^0.1"));
}

#[test]
fn a_hash_inside_a_string_is_not_a_comment() {
    let manifest =
        Manifest::parse("[package]\nname = \"a#b\"\n", None).expect("manifest should parse");
    assert_eq!(manifest.name, "a#b");
}

#[test]
fn bundles_split_back_into_their_modules() {
    let text = format!(
        "{}\n;; entry: app\n\n;; --- module app ---\n(fn main [] 1)\n\n;; --- module helper ---\n(pub fn helper [] 2)\n",
        bundle::BUNDLE_HEADER
    );
    assert!(bundle::is_bundle(&text));
    assert_eq!(bundle::bundle_entry(&text).as_deref(), Some("app"));
    let modules = bundle::read_bundle(&text);
    assert_eq!(modules.len(), 2);
    assert_eq!(modules[0].0, "app");
    assert!(modules[0].1.contains("(fn main [] 1)"));
    assert_eq!(modules[1].0, "helper");
}

#[test]
fn source_text_is_not_mistaken_for_a_bundle() {
    assert!(!bundle::is_bundle("(fn main [] 1)"));
}
