fn main() -> std::io::Result<()> {
    if cfg!(target_os = "windows") {
        // We need to set the 'longPathAware' manifest key, so that file paths with length >260 chars will work.
        // This happens sometimes since we encode IDs for duplicate files.
        let mut res = winres::WindowsResource::new();
        res.set_manifest(
            r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
<application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings xmlns:ws2="http://schemas.microsoft.com/SMI/2016/WindowsSettings">
        <ws2:longPathAware>true</ws2:longPathAware>
    </windowsSettings>
</application>
</assembly>"#);
        res.compile()?;
    }
    Ok(())
}
