fn main() {
    // Icône de l'application dans les binaires (Explorateur, barre des tâches).
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/webcamlink.rc");
        println!("cargo:rerun-if-changed=assets/webcamlink.ico");
        let _ = embed_resource::compile("assets/webcamlink.rc", embed_resource::NONE);
    }
}
