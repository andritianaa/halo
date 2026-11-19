//! Minimal HTTPS client on top of Windows' own `Windows.Web.Http.HttpClient`: system
//! proxy and certificate store, no extra TLS dependency. Used only for the update
//! check (GitHub releases). Blocking — call from a worker thread.

use std::path::Path;

use windows::Foundation::Uri;
use windows::Storage::Streams::DataReader;
use windows::Web::Http::HttpClient;
use windows::core::HSTRING;

fn client() -> windows::core::Result<HttpClient> {
    let c = HttpClient::new()?;
    let h = c.DefaultRequestHeaders()?;
    // GitHub's API rejects requests without a User-Agent.
    h.UserAgent()?.ParseAdd(&HSTRING::from(format!("Halo/{} (update check)", env!("CARGO_PKG_VERSION"))))?;
    h.Accept()?.ParseAdd(&HSTRING::from("application/vnd.github+json"))?;
    Ok(c)
}

/// GET `url` as text.
pub fn get_string(url: &str) -> windows::core::Result<String> {
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    Ok(client()?.GetStringAsync(&uri)?.get()?.to_string())
}

/// Download `url` to `dest` (whole body in memory: fine for an installer of a few MB).
pub fn download(url: &str, dest: &Path) -> windows::core::Result<u64> {
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    let buf = client()?.GetBufferAsync(&uri)?.get()?;
    let len = buf.Length()?;
    let reader = DataReader::FromBuffer(&buf)?;
    let mut bytes = vec![0u8; len as usize];
    reader.ReadBytes(&mut bytes)?;
    std::fs::write(dest, &bytes).map_err(|e| windows::core::Error::new(windows::core::HRESULT(0x8007_0005_u32 as i32), e.to_string()))?;
    Ok(len as u64)
}
