fn main() {
    // Gives the executable its own icon, which is what Explorer, the taskbar and a
    // pinned shortcut show when the window has not set one yet.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/rhumb.rc");
        let _ = embed_resource::compile("assets/rhumb.rc", embed_resource::NONE);
    }
}
