fn main() {
    let cargo_version =
        std::env::var("CARGO_PKG_VERSION").expect("Cargo must provide CARGO_PKG_VERSION");
    let display_version = cargo_version.replace('-', ".");
    println!("cargo:rustc-env=IEUM_DISPLAY_VERSION={display_version}");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
