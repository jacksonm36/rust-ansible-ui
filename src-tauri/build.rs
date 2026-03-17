fn main() {
    // Skip Tauri build when building only the server binary (no icons needed).
    if std::env::var("CARGO_FEATURE_SERVER_ONLY").is_err() {
        tauri_build::build();
    }
}
