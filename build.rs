// Windows: embed the icon as resource 1, which gpui loads for the window and
// Explorer shows for the exe. Needs a Windows host (rc.exe from the MSVC tools).
fn main() {
    println!("cargo:rerun-if-changed=tent.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("tent.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
