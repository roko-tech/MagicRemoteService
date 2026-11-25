fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    // Icon for the exe and the tray, plus the manifest (per-monitor DPI awareness)
    embed_resource::compile("resources/app.rc", embed_resource::NONE).manifest_required().unwrap();
}
