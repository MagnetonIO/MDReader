fn main() {
    // No renderer-invocable filesystem commands. Native app authority prepares
    // an immutable copy, then calls this platform handle directly.
    tauri_plugin::Builder::new(&[])
        .android_path("android")
        .ios_path("ios")
        .build();
}
