fn main() {
    println!("cargo:rerun-if-env-changed=TENT_PAYLOAD");
    match std::env::var("TENT_PAYLOAD") {
        Ok(p) => println!("cargo:rerun-if-changed={p}"),
        Err(_) => panic!("TENT_PAYLOAD must point at a zstd-compressed tent binary; build with ./dist.sh"),
    }
    // Windows: give the setup exe tent's icon (needs rc.exe, so a Windows host).
    println!("cargo:rerun-if-changed=tent-setup.rc");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("tent-setup.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
