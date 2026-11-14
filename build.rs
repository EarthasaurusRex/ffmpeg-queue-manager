fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("src-tauri/icons/icon.ico");
        let _ = res.compile();
    }
}
