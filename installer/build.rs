fn main() {
    println!("cargo:rerun-if-env-changed=TENT_PAYLOAD");
    match std::env::var("TENT_PAYLOAD") {
        Ok(p) => println!("cargo:rerun-if-changed={p}"),
        Err(_) => panic!("TENT_PAYLOAD must point at a zstd-compressed tent binary; build with ./dist.sh"),
    }
}
