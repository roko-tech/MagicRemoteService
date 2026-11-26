fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=resources/MagicRemoteService.ico");
    // Icon for the exe and the tray, plus the manifest (per-monitor DPI awareness)
    slint_build::compile("ui/app.slint").unwrap();
    // The same exe is the SYSTEM service: load the window's DLLs only when a window opens
    for dll in [
        "dwrite.dll",
        "uiautomationcore.dll",
        "imm32.dll",
        "uxtheme.dll",
        "dwmapi.dll",
        "comctl32.dll",
        "shlwapi.dll",
        "gdi32.dll",
        "combase.dll",
        "api-ms-win-core-winrt-error-l1-1-0.dll",
    ] {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
    embed_resource::compile("resources/app.rc", embed_resource::NONE).manifest_required().unwrap();
}
