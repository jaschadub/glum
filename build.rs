fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/glum.ico");
        if let Err(e) = res.compile() {
            eprintln!("warning: failed to embed Windows icon: {e}");
        }
    }
}
