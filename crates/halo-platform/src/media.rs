//! System media session (any player: Spotify, browsers, Groove…) via WinRT
//! `GlobalSystemMediaTransportControlsSessionManager`. Blocking calls — use from a
//! worker thread.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// AppUserModelId of the source (e.g. `Spotify.exe`).
    pub source: String,
    pub playing: bool,
    pub position_s: f64,
    pub duration_s: f64,
    /// Encoded album art (PNG/JPEG) when the player exposes one.
    pub art: Option<Vec<u8>>,
}

pub struct Media {
    manager: Manager,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Toggle,
    Next,
    Previous,
}

fn ticks_to_s(t: i64) -> f64 {
    t as f64 / 10_000_000.0
}

impl Media {
    pub fn new() -> Option<Self> {
        let manager = Manager::RequestAsync().ok()?.get().ok()?;
        Some(Self { manager })
    }

    fn session(&self) -> Option<Session> {
        self.manager.GetCurrentSession().ok()
    }

    /// Current session state; `with_art` also fetches the thumbnail (slower).
    pub fn now_playing(&self, with_art: bool) -> Option<NowPlaying> {
        let s = self.session()?;
        let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
        let info = s.GetPlaybackInfo().ok()?;
        let tl = s.GetTimelineProperties().ok();
        let art = if with_art { props.Thumbnail().ok().and_then(|r| read_stream(&r)) } else { None };
        Some(NowPlaying {
            title: props.Title().map(|h| h.to_string()).unwrap_or_default(),
            artist: props.Artist().map(|h| h.to_string()).unwrap_or_default(),
            album: props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default(),
            source: s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default(),
            playing: info.PlaybackStatus().ok() == Some(Status::Playing),
            position_s: tl.as_ref().and_then(|t| t.Position().ok()).map(|p| ticks_to_s(p.Duration)).unwrap_or(0.0),
            duration_s: tl.as_ref().and_then(|t| t.EndTime().ok()).map(|p| ticks_to_s(p.Duration)).unwrap_or(0.0),
            art,
        })
    }

    pub fn send(&self, cmd: Command) -> bool {
        let Some(s) = self.session() else { return false };
        let op = match cmd {
            Command::Toggle => s.TryTogglePlayPauseAsync(),
            Command::Next => s.TrySkipNextAsync(),
            Command::Previous => s.TrySkipPreviousAsync(),
        };
        op.ok().and_then(|o| o.get().ok()).unwrap_or(false)
    }
}

fn read_stream(r: &windows::Storage::Streams::IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = r.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 8 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    /// Read-only: queries the current session if any (never sends commands).
    #[test]
    fn query_current_session() {
        if let Some(m) = super::Media::new() {
            let np = m.now_playing(false);
            println!("now playing: {:?}", np.map(|n| (n.source, n.playing)));
        }
    }
}
