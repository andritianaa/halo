//! M13 — Screen recorder: a region or monitor to H.264 MP4 (Media Foundation `IMFSinkWriter`),
//! optionally with system audio and/or microphone mixed into one AAC track.
//!
//! One worker thread per recording owns COM (MTA), Media Foundation and the sink writer: it
//! grabs frames with GDI at the requested fps, paced on a pausable clock (late frames are
//! dropped, timestamps stay on wall-clock time; paused time is cut out), and interleaves
//! 48 kHz PCM from the shared audio mixer. `stop()` finalises the MP4 (blocks briefly while
//! the encoder drains). Uses the software H.264 encoder (no D3D device manager).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use halo_core::overlay::Rect;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::HSTRING;

use crate::audio::{AudioInput, CHANNELS, RATE, Source, WRITE_DELAY, to_i16};
use crate::clock::{Pacer, PauseClock, audio_time_hns, frame_time_hns, owed_frames};

pub const MANIFEST: Manifest = Manifest {
    id: "recorder",
    cdc: "M13",
    name_key: "module.recorder",
    category: Category::Capture,
    icon: "video",
    permissions: &[Permission::ScreenCapture, Permission::Microphone],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioOpts {
    pub mic: bool,
    pub system: bool,
}

impl AudioOpts {
    pub fn any(self) -> bool {
        self.mic || self.system
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoState {
    pub recording: bool,
    pub paused: bool,
    pub elapsed: Duration,
    /// Output path of the current (or last) recording.
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Pure helpers.

/// H.264 needs even dimensions: shrink odd sizes by one pixel (min 2×2).
pub fn even_size(r: Rect) -> Rect {
    Rect::new(r.x, r.y, (r.w & !1).max(2), (r.h & !1).max(2))
}

/// ~0.1 bit per pixel per frame, clamped to 1–40 Mbit/s.
pub fn bitrate(w: u32, h: u32, fps: u32) -> u32 {
    ((w as u64 * h as u64 * fps as u64) / 10).clamp(1_000_000, 40_000_000) as u32
}

/// Top-down RGBA → bottom-up BGRX (MFVideoFormat_RGB32 with negative default stride).
pub fn rgba_to_rgb32_bottom_up(src: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    let row = w * 4;
    out.resize(row * h, 0);
    for y in 0..h {
        let s = &src[y * row..(y + 1) * row];
        let d = &mut out[(h - 1 - y) * row..(h - y) * row];
        for (dp, sp) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
            dp[0] = sp[2];
            dp[1] = sp[1];
            dp[2] = sp[0];
            dp[3] = 255;
        }
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

// ---------------------------------------------------------------------------------------------
// Media Foundation.

/// COM (MTA) + Media Foundation for the current thread, released on drop.
pub struct MfSession {
    com: bool,
}

impl MfSession {
    pub fn new() -> anyhow::Result<Self> {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        Ok(Self { com })
    }
}

impl Drop for MfSession {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

/// H.264 (+ optional AAC) MP4 writer. Create, write and finalise on the same thread
/// (inside an [`MfSession`]).
pub struct Mp4Writer {
    w: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
    width: u32,
    height: u32,
    fps: u32,
    buf: Vec<u8>,
}

impl Mp4Writer {
    pub fn create(path: &Path, width: u32, height: u32, fps: u32, with_audio: bool) -> anyhow::Result<Self> {
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) || width == 0 || height == 0 {
            bail!("frame size must be even and non-zero ({width}×{height})");
        }
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs = attrs.ok_or_else(|| anyhow!("MFCreateAttributes"))?;
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            let w = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)?;

            let size = pack(width, height);
            let rate = pack(fps, 1);
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(width, height, fps))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            let video = w.AddStream(&out)?;

            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, (-((width * 4) as i32)) as u32)?; // bottom-up
            inp.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            w.SetInputMediaType(video, &inp, None)?;

            let audio = if with_audio {
                let ch = CHANNELS as u32;
                let out = MFCreateMediaType()?;
                out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
                out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch)?;
                out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbit/s
                let a = w.AddStream(&out)?;
                let inp = MFCreateMediaType()?;
                inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
                inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch)?;
                inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, ch * 2)?;
                inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * ch * 2)?;
                w.SetInputMediaType(a, &inp, None)?;
                Some(a)
            } else {
                None
            };
            w.BeginWriting()?;
            Ok(Self { w, video, audio, width, height, fps, buf: Vec::new() })
        }
    }

    fn write(&self, stream: u32, bytes: &[u8], time: i64, duration: i64) -> anyhow::Result<()> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            self.w.WriteSample(stream, &sample)?;
        }
        Ok(())
    }

    /// One top-down RGBA frame (`width × height`) in frame slot `slot`.
    pub fn write_frame(&mut self, rgba: &[u8], slot: u64) -> anyhow::Result<()> {
        let (w, h) = (self.width as usize, self.height as usize);
        if rgba.len() != w * h * 4 {
            bail!("frame size mismatch");
        }
        let mut buf = std::mem::take(&mut self.buf);
        rgba_to_rgb32_bottom_up(rgba, w, h, &mut buf);
        let t = frame_time_hns(slot, self.fps);
        let r = self.write(self.video, &buf, t, frame_time_hns(slot + 1, self.fps) - t);
        self.buf = buf;
        r
    }

    /// Interleaved stereo f32 starting at audio frame `start_frame` (48 kHz).
    pub fn write_audio(&mut self, stereo: &[f32], start_frame: u64) -> anyhow::Result<()> {
        let Some(a) = self.audio else { return Ok(()) };
        if stereo.is_empty() {
            return Ok(());
        }
        let bytes: Vec<u8> = stereo.iter().flat_map(|s| to_i16(*s).to_le_bytes()).collect();
        let frames = (stereo.len() / 2) as u64;
        let t = audio_time_hns(start_frame, RATE);
        self.write(a, &bytes, t, audio_time_hns(start_frame + frames, RATE) - t)
    }

    pub fn finalize(self) -> anyhow::Result<()> {
        unsafe { self.w.Finalize()? };
        Ok(())
    }
}

/// Duration of a media file via `IMFSourceReader` (call inside an [`MfSession`]).
pub fn media_duration(path: &Path) -> anyhow::Result<Duration> {
    unsafe {
        let r = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let pv = r.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)?;
        let hns = pv.Anonymous.Anonymous.Anonymous.uhVal;
        Ok(Duration::from_nanos(hns.saturating_mul(100)))
    }
}

// ---------------------------------------------------------------------------------------------
// Recorder.

struct Active {
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    join: JoinHandle<anyhow::Result<()>>,
}

struct Inner {
    enabled: bool,
    folder: PathBuf,
    active: Option<Active>,
    clock: Option<PauseClock>,
    path: Option<PathBuf>,
    error: Option<String>,
}

#[derive(Clone)]
pub struct VideoHandle(Arc<Mutex<Inner>>);

impl Default for VideoHandle {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Inner {
            enabled: false,
            folder: crate::user_folder("Videos"),
            active: None,
            clock: None,
            path: None,
            error: None,
        })))
    }
}

impl VideoHandle {
    pub fn folder(&self) -> PathBuf {
        self.0.lock().unwrap().folder.clone()
    }

    pub fn set_folder(&self, dir: PathBuf) {
        self.0.lock().unwrap().folder = dir;
    }

    /// Start recording `rect` (physical px; odd sizes are trimmed by 1 px) at `fps` (1–60,
    /// typically 15 or 30). Blocks until the encoder and audio devices are ready (< ~0.5 s).
    pub fn start(&self, rect: Rect, fps: u32, audio: AudioOpts, effects: crate::Effects) -> anyhow::Result<PathBuf> {
        let g = self.0.lock().unwrap();
        if !g.enabled {
            bail!("screen recorder module is disabled");
        }
        if g.active.is_some() {
            bail!("already recording");
        }
        if rect.w < 2 || rect.h < 2 {
            bail!("region too small");
        }
        let fps = fps.clamp(1, 60);
        let path = crate::output_path(&g.folder, "Recording", "mp4")?;
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (inner, p, s, out) = (self.0.clone(), paused.clone(), stop.clone(), path.clone());
        let join = std::thread::Builder::new().name("halo-recorder".into()).spawn(move || {
            let r = run(&inner, even_size(rect), fps, audio, effects, &out, p, s, &tx);
            if let Err(e) = &r {
                log::error!("screen recording failed: {e}");
                inner.lock().unwrap().error = Some(e.to_string());
                let _ = tx.send(Err(anyhow!("{e}")));
            }
            r
        })?;
        drop(g);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = join.join();
                let _ = std::fs::remove_file(&path);
                self.0.lock().unwrap().clock = None;
                return Err(e);
            }
            Err(_) => {
                let _ = join.join();
                bail!("recorder thread exited");
            }
        }
        let mut g = self.0.lock().unwrap();
        g.path = Some(path.clone());
        g.error = None;
        g.active = Some(Active { paused, stop, join });
        Ok(path)
    }

    pub fn pause(&self) {
        let mut g = self.0.lock().unwrap();
        let Inner { active, clock, .. } = &mut *g;
        if let (Some(a), Some(c)) = (active.as_ref(), clock.as_mut()) {
            c.pause(Instant::now());
            a.paused.store(true, Ordering::Relaxed);
        }
    }

    pub fn resume(&self) {
        let mut g = self.0.lock().unwrap();
        let Inner { active, clock, .. } = &mut *g;
        if let (Some(a), Some(c)) = (active.as_ref(), clock.as_mut()) {
            c.resume(Instant::now());
            a.paused.store(false, Ordering::Relaxed);
        }
    }

    /// Stop and finalise the MP4; returns its path.
    pub fn stop(&self) -> anyhow::Result<PathBuf> {
        let (a, path) = {
            let mut g = self.0.lock().unwrap();
            (g.active.take().ok_or_else(|| anyhow!("not recording"))?, g.path.clone())
        };
        a.stop.store(true, Ordering::Relaxed);
        let r = a.join.join().map_err(|_| anyhow!("recorder thread panicked"))?;
        self.0.lock().unwrap().clock = None;
        r?;
        path.ok_or_else(|| anyhow!("no output"))
    }

    pub fn state(&self) -> VideoState {
        let g = self.0.lock().unwrap();
        let running = g.active.as_ref().is_some_and(|a| !a.join.is_finished());
        VideoState {
            recording: running,
            paused: running && g.clock.is_some_and(|c| c.is_paused()),
            elapsed: g.clock.map_or(Duration::ZERO, |c| c.elapsed(Instant::now())),
            path: g.path.clone(),
            error: g.error.clone(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    inner: &Arc<Mutex<Inner>>,
    rect: Rect,
    fps: u32,
    audio: AudioOpts,
    effects: crate::Effects,
    path: &Path,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    ready: &mpsc::Sender<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let _mf = MfSession::new()?;
    let input = if audio.any() { Some(AudioInput::open(Source { mic: audio.mic, system: audio.system }, paused.clone())?) } else { None };
    let mut writer = Mp4Writer::create(path, rect.w as u32, rect.h as u32, fps, input.is_some())?;
    inner.lock().unwrap().clock = Some(PauseClock::start(Instant::now()));
    let _ = ready.send(Ok(()));

    let elapsed = || inner.lock().unwrap().clock.map_or(Duration::ZERO, |c| c.elapsed(Instant::now()));
    let mut pacer = Pacer::new(fps);
    let mut audio_written = 0u64;
    let mut last_frame: Option<Vec<u8>> = None;
    let mut clicks = crate::effects::ClickTracker::default();
    let scale = halo_platform::monitor_for(0).scale;
    loop {
        // Sample the buttons every iteration (≤ 10 ms) so short clicks are not missed.
        if effects.clicks != crate::ClickStyle::Off {
            let down = halo_platform::key_down(0x01) || halo_platform::key_down(0x02);
            clicks.sample(Instant::now(), down, halo_platform::cursor_pos().unwrap_or_default());
        }
        let stopping = stop.load(Ordering::Relaxed);
        if !paused.load(Ordering::Relaxed) || stopping {
            let now = elapsed();
            if !stopping {
                if let Some(slot) = pacer.due(now) {
                    // A failed grab (secure desktop, display change) repeats the last frame.
                    let grabbed = if effects.cursor { halo_platform::capture::grab_with_cursor(rect) } else { halo_platform::capture::grab(rect) };
                    match grabbed {
                        Some(mut img) => {
                            let (w, h) = (img.width as usize, img.height as usize);
                            clicks.draw(Instant::now(), effects.clicks, &mut img.rgba, w, h, (rect.x, rect.y), scale);
                            writer.write_frame(&img.rgba, slot)?;
                            last_frame = Some(img.rgba);
                        }
                        None => {
                            if let Some(f) = &last_frame {
                                writer.write_frame(f, slot)?;
                            }
                        }
                    }
                }
            }
            if let Some(input) = &input {
                let delay = if stopping { Duration::ZERO } else { WRITE_DELAY };
                let n = owed_frames(now, delay, RATE, audio_written);
                if n > 0 {
                    let buf = input.mixer.lock().unwrap().pull(n as usize);
                    writer.write_audio(&buf, audio_written)?;
                    audio_written += n;
                }
            }
        }
        if stopping {
            break;
        }
        let wait = pacer.next_at().saturating_sub(elapsed()).clamp(Duration::from_millis(1), Duration::from_millis(10));
        std::thread::sleep(wait);
    }
    drop(input);
    if last_frame.is_none() {
        bail!("no frame could be captured");
    }
    writer.finalize()
}

pub struct ScreenRecorder {
    handle: VideoHandle,
}

impl ScreenRecorder {
    pub fn new() -> Self {
        Self { handle: VideoHandle::default() }
    }
    pub fn handle(&self) -> VideoHandle {
        self.handle.clone()
    }
}

impl Default for ScreenRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for ScreenRecorder {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        self.handle.0.lock().unwrap().enabled = true;
        Ok(())
    }
    fn stop(&mut self) {
        let _ = self.handle.stop();
        self.handle.0.lock().unwrap().enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_sizes_and_bitrate() {
        assert_eq!(even_size(Rect::new(1, 2, 321, 241)), Rect::new(1, 2, 320, 240));
        assert_eq!(even_size(Rect::new(0, 0, 1, 1)), Rect::new(0, 0, 2, 2));
        assert_eq!(bitrate(1920, 1080, 30), 6_220_800);
        assert_eq!(bitrate(320, 240, 15), 1_000_000);
    }

    #[test]
    fn rgb32_conversion_flips_and_swaps() {
        // 1×2 image: top red, bottom blue.
        let src = [255, 0, 0, 255, 0, 0, 255, 255];
        let mut out = vec![];
        rgba_to_rgb32_bottom_up(&src, 1, 2, &mut out);
        // First row in memory = bottom (blue) as BGRX.
        assert_eq!(out, [255, 0, 0, 255, 0, 0, 255, 255]);
        let src = [10, 20, 30, 0];
        rgba_to_rgb32_bottom_up(&src, 1, 1, &mut out);
        assert_eq!(out, [30, 20, 10, 255]);
    }

    /// Synthetic frames + tone (no screen, no device): H.264 + AAC muxing and duration.
    #[test]
    fn mp4_writer_synthetic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.mp4");
        let _mf = MfSession::new().unwrap();
        let (w, h, fps) = (160u32, 120u32, 15u32);
        let mut wr = Mp4Writer::create(&path, w, h, fps, true).unwrap();
        let frame: Vec<u8> = (0..w * h).flat_map(|i| [(i % 256) as u8, 128, 64, 255]).collect();
        for slot in 0..30 {
            wr.write_frame(&frame, slot).unwrap();
        }
        let tone: Vec<f32> = (0..RATE as usize * 2).flat_map(|i| [((i as f32) * 0.05).sin() * 0.2; 2]).collect();
        for (k, chunk) in tone.chunks(4800).enumerate() {
            wr.write_audio(chunk, (k * 2400) as u64).unwrap();
        }
        wr.finalize().unwrap();
        let d = media_duration(&path).unwrap();
        assert!((1.8..2.3).contains(&d.as_secs_f64()), "{d:?}");
        assert!(std::fs::metadata(&path).unwrap().len() > 1024);
    }

    /// Synthetic frame (top white, bottom black) → MP4 → decoded back to RGB32: must be upright.
    #[test]
    fn mp4_orientation_is_upright() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orient.mp4");
        let _mf = MfSession::new().unwrap();
        let (w, h) = (160u32, 120u32);
        let frame: Vec<u8> = (0..h).flat_map(|y| [if y < h / 2 { 255 } else { 0 }; 4].repeat(w as usize)).collect();
        let mut wr = Mp4Writer::create(&path, w, h, 15, false).unwrap();
        for slot in 0..15 {
            wr.write_frame(&frame, slot).unwrap();
        }
        wr.finalize().unwrap();
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1).unwrap();
            let attrs = attrs.unwrap();
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).unwrap();
            let r = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs).unwrap();
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let mt = MFCreateMediaType().unwrap();
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).unwrap();
            r.SetCurrentMediaType(stream, None, &mt).unwrap();
            let cur = r.GetCurrentMediaType(stream).unwrap();
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).unwrap();
            let (dw, dh) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(-((dw * 4) as i32));
            let mut sample = None;
            for _ in 0..50 {
                let mut flags = 0u32;
                r.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)).unwrap();
                if sample.is_some() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            let buf = sample.expect("decoded frame").ConvertToContiguousBuffer().unwrap();
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len)).unwrap();
            let data = std::slice::from_raw_parts(p, len as usize);
            let pitch = stride.unsigned_abs() as usize;
            // Row `y` counted from the top of the picture.
            let row = |y: usize| if stride > 0 { y * pitch } else { (dh - 1 - y) * pitch };
            let top = data[row(10) + (dw / 2) * 4 + 1];
            let bottom = data[row(h as usize - 10) + (dw / 2) * 4 + 1];
            buf.Unlock().unwrap();
            assert!(top > 200 && bottom < 50, "top {top} bottom {bottom} ({dw}×{dh}, stride {stride})");
        }
    }

    /// Real screen, privacy-safe: ~2 s of a 320×240 region at 15 fps into a temp dir; only the
    /// size and duration are checked, then the file is deleted. Audio left off (silent test).
    #[test]
    fn record_two_seconds_to_temp() {
        let dir = tempfile::tempdir().unwrap();
        let h = VideoHandle::default();
        h.0.lock().unwrap().enabled = true;
        h.set_folder(dir.path().to_path_buf());
        let vs = halo_platform::capture::virtual_screen();
        let path = h.start(Rect::new(vs.x, vs.y, 320, 240), 15, AudioOpts::default(), crate::Effects::default()).unwrap();
        std::thread::sleep(Duration::from_millis(1000));
        h.pause();
        assert!(h.state().paused);
        std::thread::sleep(Duration::from_millis(500));
        h.resume();
        std::thread::sleep(Duration::from_millis(1000));
        assert!(h.state().recording);
        let out = h.stop().unwrap();
        assert_eq!(out, path);
        assert!(std::fs::metadata(&out).unwrap().len() > 1024);
        let _mf = MfSession::new().unwrap();
        let d = media_duration(&out).unwrap().as_secs_f64();
        assert!((1.6..2.4).contains(&d), "duration {d} (pause must be cut out)");
        std::fs::remove_file(&out).unwrap();
    }

    /// Real screen + loopback audio (silent capture, nothing played), skipped without a device.
    #[test]
    fn record_with_system_audio_to_temp() {
        if cpal::traits::HostTrait::default_output_device(&cpal::default_host()).is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let h = VideoHandle::default();
        h.0.lock().unwrap().enabled = true;
        h.set_folder(dir.path().to_path_buf());
        let vs = halo_platform::capture::virtual_screen();
        let Ok(_) = h.start(Rect::new(vs.x, vs.y, 320, 240), 15, AudioOpts { mic: false, system: true }, crate::Effects::default()) else {
            eprintln!("loopback unavailable, skipping");
            return;
        };
        std::thread::sleep(Duration::from_millis(1500));
        let out = h.stop().unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() > 1024);
        std::fs::remove_file(&out).unwrap();
    }
}
