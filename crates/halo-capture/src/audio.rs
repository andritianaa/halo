//! M14 — Audio recorder: microphone and/or system audio (WASAPI loopback via cpal), mixed to
//! one 48 kHz stereo stream, saved as 16-bit WAV or FLAC in `%USERPROFILE%\Music\Halo`.
//!
//! Capture runs on cpal's WASAPI threads; a writer thread owned by the recording pulls from
//! the mixer every 20 ms, driven by a pausable clock. FLAC is produced at `stop()` by
//! encoding the temporary WAV (pure-Rust `flacenc`), so `stop()` blocks for that encode
//! (roughly 1 s per 2–3 min of audio); WAV stop is immediate.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};

use crate::clock::{PauseClock, owed_frames};

pub const MANIFEST: Manifest = Manifest {
    id: "audiorec",
    cdc: "M14",
    name_key: "module.audiorec",
    category: Category::Capture,
    icon: "mic",
    permissions: &[Permission::Microphone],
    opt_in: false,
    default_enabled: true,
};

/// Mixer / file sample rate (also accepted by the Media Foundation AAC encoder).
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// How far the writer trails the clock so capture buffers can fill.
pub(crate) const WRITE_DELAY: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Source {
    pub mic: bool,
    pub system: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Wav,
    Flac,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Flac => "flac",
        }
    }
}

/// Input level of the last mixed chunk, 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level {
    pub rms: f32,
    pub peak: f32,
}

pub fn level(samples: &[f32]) -> Level {
    if samples.is_empty() {
        return Level::default();
    }
    let (mut sum, mut peak) = (0f64, 0f32);
    for &s in samples {
        sum += (s as f64) * (s as f64);
        peak = peak.max(s.abs());
    }
    Level { rms: ((sum / samples.len() as f64).sqrt() as f32).min(1.0), peak: peak.min(1.0) }
}

// ---------------------------------------------------------------------------------------------
// Pure DSP: channel mapping, linear resampling, mixing.

/// Interleaved `channels` → interleaved stereo (mono duplicated, >2 channels: front L/R).
pub fn to_stereo(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    let ch = channels.max(1);
    for f in input.chunks_exact(ch) {
        let l = f[0];
        let r = if ch > 1 { f[1] } else { l };
        out.push(l);
        out.push(r);
    }
}

/// Streaming linear-interpolation resampler for interleaved stereo.
#[derive(Clone, Debug)]
pub struct Resampler {
    step: f64,
    t: f64,
    prev: [f32; 2],
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { step: from.max(1) as f64 / to.max(1) as f64, t: 0.0, prev: [0.0; 2] }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len() / 2;
        if n == 0 {
            return;
        }
        if self.step == 1.0 && self.t == 0.0 {
            out.extend_from_slice(&input[..n * 2]);
            return;
        }
        // Virtual sequence: s0 = prev, s(k+1) = input[k]; output at positions t, t+step, …
        let at = |i: usize| if i == 0 { self.prev } else { [input[(i - 1) * 2], input[(i - 1) * 2 + 1]] };
        while self.t < n as f64 {
            let i = self.t.floor() as usize;
            let f = (self.t - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.push(a[0] + (b[0] - a[0]) * f);
            out.push(a[1] + (b[1] - a[1]) * f);
            self.t += self.step;
        }
        self.t -= n as f64;
        self.prev = [input[(n - 1) * 2], input[(n - 1) * 2 + 1]];
    }
}

/// Sums several stereo sources. A starved source contributes silence; a source running
/// ahead is capped at `max_backlog` frames (oldest dropped) so clocks can't drift apart.
#[derive(Debug)]
pub struct Mixer {
    sources: Vec<VecDeque<f32>>,
    max_backlog: usize,
}

impl Mixer {
    pub fn new(sources: usize, max_backlog_frames: usize) -> Self {
        Self { sources: vec![VecDeque::new(); sources], max_backlog: max_backlog_frames * 2 }
    }

    pub fn push(&mut self, src: usize, stereo: &[f32]) {
        let q = &mut self.sources[src];
        q.extend(stereo.iter().copied());
        if q.len() > self.max_backlog {
            let extra = q.len() - self.max_backlog;
            q.drain(..extra);
        }
    }

    /// Exactly `frames` stereo frames.
    pub fn pull(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![0f32; frames * 2];
        for q in &mut self.sources {
            let n = q.len().min(out.len());
            for (o, s) in out.iter_mut().zip(q.drain(..n)) {
                *o += s;
            }
        }
        for o in &mut out {
            *o = o.clamp(-1.0, 1.0);
        }
        out
    }

    pub fn buffered(&self, src: usize) -> usize {
        self.sources[src].len() / 2
    }
}

pub fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

// ---------------------------------------------------------------------------------------------
// Device capture.

// ---------------------------------------------------------------------------------------------
// Microphone choice and test.

/// Microphone chosen in the settings (`None` = Windows default). Set by the app.
static PREFERRED_MIC: Mutex<Option<String>> = Mutex::new(None);

/// Use this microphone (by name) for recordings; `None` follows the Windows default.
pub fn set_microphone(name: Option<String>) {
    *PREFERRED_MIC.lock().unwrap_or_else(|e| e.into_inner()) = name.filter(|n| !n.is_empty());
}

/// Names of the available microphones (input devices).
pub fn microphones() -> Vec<String> {
    cpal::default_host().input_devices().map(|it| it.map(|d| d.to_string()).collect()).unwrap_or_default()
}

/// The chosen microphone if it is still connected, else the Windows default.
fn microphone(host: &cpal::Host) -> Option<cpal::Device> {
    let want = PREFERRED_MIC.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(name) = want {
        if let Some(d) = host.input_devices().ok().and_then(|mut it| it.find(|d| d.to_string() == name)) {
            return Some(d);
        }
        log::warn!("microphone \"{name}\" not found, using the default one");
    }
    host.default_input_device()
}

/// Live input level of a microphone, for the "Test" button in the settings.
pub struct MicTest {
    _stream: cpal::Stream,
    peak: Arc<std::sync::atomic::AtomicU32>,
}

impl MicTest {
    /// Open the chosen microphone (or `name` if given) and start metering.
    pub fn start(name: Option<&str>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let dev = match name {
            Some(n) => host.input_devices()?.find(|d| d.to_string() == n).ok_or_else(|| anyhow!("microphone not found"))?,
            None => microphone(&host).ok_or_else(|| anyhow!("no microphone"))?,
        };
        let cfg = dev.default_input_config().context("microphone format")?;
        let peak = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let p = peak.clone();
        let sc = cfg.config();
        use cpal::SampleFormat as F;
        let stream = match cfg.sample_format() {
            F::F32 => meter::<f32>(&dev, &sc, p),
            F::I16 => meter::<i16>(&dev, &sc, p),
            F::I32 => meter::<i32>(&dev, &sc, p),
            F::U8 => meter::<u8>(&dev, &sc, p),
            F::U16 => meter::<u16>(&dev, &sc, p),
            F::F64 => meter::<f64>(&dev, &sc, p),
            f => Err(anyhow!("unsupported sample format {f:?}")),
        }?;
        stream.play()?;
        Ok(Self { _stream: stream, peak })
    }

    /// Peak of the last buffer, 0..1 (decays when silent).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }
}

fn meter<T>(dev: &cpal::Device, cfg: &cpal::StreamConfig, peak: Arc<std::sync::atomic::AtomicU32>) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let stream = dev.build_input_stream(
        *cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let p = data.iter().map(|&s| <f32 as cpal::FromSample<T>>::from_sample_(s).abs()).fold(0f32, f32::max).min(1.0);
            // Fast attack, slow release, like a VU meter.
            let prev = f32::from_bits(peak.load(Ordering::Relaxed));
            let v = if p >= prev { p } else { prev * 0.85 + p * 0.15 };
            peak.store(v.to_bits(), Ordering::Relaxed);
        },
        |e| log::warn!("microphone test stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Open mic and/or loopback streams feeding a shared [`Mixer`] at [`RATE`] stereo.
/// Callbacks drop data while `paused` is set. Keep the value alive to keep capturing.
pub(crate) struct AudioInput {
    _streams: Vec<cpal::Stream>,
    pub mixer: Arc<Mutex<Mixer>>,
}

impl AudioInput {
    pub fn open(src: Source, paused: Arc<AtomicBool>) -> anyhow::Result<Self> {
        if !src.mic && !src.system {
            bail!("no audio source selected");
        }
        let host = cpal::default_host();
        let mut devices = Vec::new();
        if src.system {
            let dev = host.default_output_device().ok_or_else(|| anyhow!("no audio output device for loopback"))?;
            let cfg = dev.default_output_config().context("loopback format")?;
            devices.push(("system", dev, cfg));
        }
        if src.mic {
            let dev = microphone(&host).ok_or_else(|| anyhow!("no microphone"))?;
            let cfg = dev.default_input_config().context("microphone format")?;
            devices.push(("mic", dev, cfg));
        }
        let mixer = Arc::new(Mutex::new(Mixer::new(devices.len(), RATE as usize)));
        let mut streams = Vec::new();
        for (idx, (name, dev, cfg)) in devices.into_iter().enumerate() {
            let sc = cfg.config();
            let (m, p) = (mixer.clone(), paused.clone());
            use cpal::SampleFormat as F;
            let stream = match cfg.sample_format() {
                F::F32 => build::<f32>(&dev, &sc, idx, m, p),
                F::I16 => build::<i16>(&dev, &sc, idx, m, p),
                F::I32 => build::<i32>(&dev, &sc, idx, m, p),
                F::U8 => build::<u8>(&dev, &sc, idx, m, p),
                F::U16 => build::<u16>(&dev, &sc, idx, m, p),
                F::F64 => build::<f64>(&dev, &sc, idx, m, p),
                f => Err(anyhow!("unsupported sample format {f:?}")),
            }
            .with_context(|| format!("open {name} stream"))?;
            stream.play().with_context(|| format!("start {name} stream"))?;
            log::info!("audio capture: {name} {} Hz × {} ch {:?}", sc.sample_rate, sc.channels, cfg.sample_format());
            streams.push(stream);
        }
        Ok(Self { _streams: streams, mixer })
    }
}

fn build<T>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    idx: usize,
    mixer: Arc<Mutex<Mixer>>,
    paused: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let ch = cfg.channels as usize;
    let mut rs = Resampler::new(cfg.sample_rate, RATE);
    let (mut f, mut st, mut out) = (Vec::<f32>::new(), Vec::new(), Vec::new());
    let stream = dev.build_input_stream(
        *cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if paused.load(Ordering::Relaxed) {
                return;
            }
            f.clear();
            f.extend(data.iter().map(|&s| <f32 as cpal::FromSample<T>>::from_sample_(s)));
            st.clear();
            to_stereo(&f, ch, &mut st);
            out.clear();
            rs.process(&st, &mut out);
            if let Ok(mut m) = mixer.lock() {
                m.push(idx, &out);
            }
        },
        |e| log::warn!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

// ---------------------------------------------------------------------------------------------
// Files: trim, FLAC.

/// Frame range kept by a trim of `[start_s, end_s)` (clamped; `end_s <= start_s` → empty).
pub fn trim_range(total_frames: u64, rate: u32, start_s: f64, end_s: f64) -> Range<u64> {
    let f = |s: f64| ((s.max(0.0) * rate as f64).round() as u64).min(total_frames);
    let (a, b) = (f(start_s), f(end_s));
    a..b.max(a)
}

/// Copy `[start_s, end_s)` of a WAV into `"<stem> (trim).wav"` next to it; returns the new path.
pub fn trim_wav(path: &Path, start_s: f64, end_s: f64) -> anyhow::Result<PathBuf> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    let ch = spec.channels as u64;
    let range = trim_range(r.duration() as u64, spec.sample_rate, start_s, end_s);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    let out = crate::unique_path(dir, &format!("{stem} (trim).wav"));
    let mut w = hound::WavWriter::create(&out, spec)?;
    r.seek(range.start as u32)?;
    let n = ((range.end - range.start) * ch) as usize;
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in r.samples::<f32>().take(n) {
                w.write_sample(s?)?;
            }
        }
        hound::SampleFormat::Int => {
            for s in r.samples::<i32>().take(n) {
                w.write_sample(s?)?;
            }
        }
    }
    w.finalize()?;
    Ok(out)
}

/// Streams 16/24-bit integer WAV samples into flacenc.
struct WavSource<R: std::io::Read> {
    reader: hound::WavReader<R>,
    buf: Vec<i32>,
}

impl<R: std::io::Read> flacenc::source::Source for WavSource<R> {
    fn channels(&self) -> usize {
        self.reader.spec().channels as usize
    }
    fn bits_per_sample(&self) -> usize {
        self.reader.spec().bits_per_sample as usize
    }
    fn sample_rate(&self) -> usize {
        self.reader.spec().sample_rate as usize
    }
    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let ch = self.channels();
        self.buf.clear();
        self.buf.extend(self.reader.samples::<i32>().take(block_size * ch).map_while(Result::ok));
        let frames = self.buf.len() / ch;
        self.buf.truncate(frames * ch);
        dest.fill_interleaved(&self.buf)?;
        Ok(frames)
    }
}

/// Encode an integer PCM WAV to FLAC.
pub fn wav_to_flac(wav: &Path, flac: &Path) -> anyhow::Result<()> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let reader = hound::WavReader::open(wav)?;
    if reader.spec().sample_format != hound::SampleFormat::Int {
        bail!("FLAC needs integer PCM");
    }
    let config = flacenc::config::Encoder::default().into_verified().map_err(|e| anyhow!("flac config: {e:?}"))?;
    let src = WavSource { reader, buf: Vec::new() };
    let stream = flacenc::encode_with_fixed_block_size(&config, src, config.block_size)
        .map_err(|e| anyhow!("flac encode: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| anyhow!("flac write: {e:?}"))?;
    std::fs::write(flac, sink.as_slice())?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Recorder handle + module.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioState {
    pub recording: bool,
    pub paused: bool,
    pub elapsed: Duration,
    pub level: Level,
    /// Final output path of the current (or last) recording.
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}

struct Active {
    clock: PauseClock,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    join: JoinHandle<anyhow::Result<PathBuf>>,
}

struct Inner {
    enabled: bool,
    folder: PathBuf,
    active: Option<Active>,
    level: Level,
    path: Option<PathBuf>,
    error: Option<String>,
}

#[derive(Clone)]
pub struct AudioHandle(Arc<Mutex<Inner>>);

impl Default for AudioHandle {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Inner {
            enabled: false,
            folder: crate::user_folder("Music"),
            active: None,
            level: Level::default(),
            path: None,
            error: None,
        })))
    }
}

impl AudioHandle {
    pub fn folder(&self) -> PathBuf {
        self.0.lock().unwrap().folder.clone()
    }

    pub fn set_folder(&self, dir: PathBuf) {
        self.0.lock().unwrap().folder = dir;
    }

    /// Start recording. Blocks until the devices are open (or failed), typically < 200 ms.
    pub fn start(&self, src: Source, fmt: Format) -> anyhow::Result<PathBuf> {
        let g = self.0.lock().unwrap();
        if !g.enabled {
            bail!("audio recorder module is disabled");
        }
        if g.active.is_some() {
            bail!("already recording");
        }
        let path = crate::output_path(&g.folder, "Recording", fmt.ext())?;
        let wav = match fmt {
            Format::Wav => path.clone(),
            Format::Flac => path.with_extension("flac.part.wav"),
        };
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let inner = self.0.clone();
        let (p, s, final_path) = (paused.clone(), stop.clone(), path.clone());
        let join = std::thread::Builder::new().name("halo-audiorec".into()).spawn(move || {
            let res = record(&inner, src, &wav, p, s, tx);
            let res = res.and_then(|()| {
                if fmt == Format::Flac {
                    let r = wav_to_flac(&wav, &final_path);
                    let _ = std::fs::remove_file(&wav);
                    r?;
                }
                Ok(final_path)
            });
            if let Err(e) = &res {
                inner.lock().unwrap().error = Some(e.to_string());
            }
            res
        })?;
        drop(g);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = join.join();
                return Err(e);
            }
            Err(_) => {
                return Err(join.join().map_err(|_| anyhow!("audio thread panicked"))?.err().unwrap_or_else(|| anyhow!("audio thread exited")));
            }
        }
        let mut g = self.0.lock().unwrap();
        g.path = Some(path.clone());
        g.error = None;
        g.active = Some(Active { clock: PauseClock::start(Instant::now()), paused, stop, join });
        Ok(path)
    }

    pub fn pause(&self) {
        let mut g = self.0.lock().unwrap();
        if let Some(a) = g.active.as_mut() {
            a.paused.store(true, Ordering::Relaxed);
            a.clock.pause(Instant::now());
        }
        g.level = Level::default();
    }

    pub fn resume(&self) {
        if let Some(a) = self.0.lock().unwrap().active.as_mut() {
            a.clock.resume(Instant::now());
            a.paused.store(false, Ordering::Relaxed);
        }
    }

    /// Stop and finalise the file (FLAC: encodes now, may take a moment).
    pub fn stop(&self) -> anyhow::Result<PathBuf> {
        let a = self.0.lock().unwrap().active.take().ok_or_else(|| anyhow!("not recording"))?;
        a.stop.store(true, Ordering::Relaxed);
        let res = a.join.join().map_err(|_| anyhow!("audio thread panicked"))?;
        self.0.lock().unwrap().level = Level::default();
        res
    }

    pub fn state(&self) -> AudioState {
        let g = self.0.lock().unwrap();
        let now = Instant::now();
        AudioState {
            recording: g.active.is_some(),
            paused: g.active.as_ref().is_some_and(|a| a.clock.is_paused()),
            elapsed: g.active.as_ref().map_or(Duration::ZERO, |a| a.clock.elapsed(now)),
            level: g.level,
            path: g.path.clone(),
            error: g.error.clone(),
        }
    }

    fn elapsed(&self) -> Option<Duration> {
        self.0.lock().unwrap().active.as_ref().map(|a| a.clock.elapsed(Instant::now()))
    }
}

/// Writer thread body.
fn record(
    inner: &Arc<Mutex<Inner>>,
    src: Source,
    wav: &Path,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let spec = hound::WavSpec { channels: CHANNELS, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let opened = AudioInput::open(src, paused).and_then(|input| Ok((input, hound::WavWriter::create(wav, spec)?)));
    let (input, mut writer) = match opened {
        Ok(v) => {
            let _ = ready.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(()); // already reported to start()
        }
    };
    let handle = AudioHandle(inner.clone());
    let mut written = 0u64;
    loop {
        let stopping = stop.load(Ordering::Relaxed);
        // `active` is set right after `ready`; until then elapsed is ~0.
        let elapsed = handle.elapsed().unwrap_or(Duration::ZERO);
        let delay = if stopping { Duration::ZERO } else { WRITE_DELAY };
        let n = owed_frames(elapsed, delay, RATE, written);
        if n > 0 {
            let buf = input.mixer.lock().unwrap().pull(n as usize);
            inner.lock().unwrap().level = level(&buf);
            for s in &buf {
                writer.write_sample(to_i16(*s))?;
            }
            written += n;
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(input);
    writer.finalize()?;
    Ok(())
}

pub struct AudioRecorder {
    handle: AudioHandle,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self { handle: AudioHandle::default() }
    }
    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }
}

impl Default for AudioRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for AudioRecorder {
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

    fn sine(rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn stereo_mapping() {
        let mut out = vec![];
        to_stereo(&[0.1, 0.2], 1, &mut out);
        assert_eq!(out, [0.1, 0.1, 0.2, 0.2]);
        out.clear();
        to_stereo(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 3, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.4, 0.5]);
    }

    #[test]
    fn resampler_lengths_and_dc() {
        for (from, to) in [(44_100, 48_000), (96_000, 48_000), (48_000, 48_000), (16_000, 48_000)] {
            let mut r = Resampler::new(from, to);
            let mut out = vec![];
            let chunk = vec![0.25f32; 2 * 441];
            for _ in 0..100 {
                r.process(&chunk, &mut out); // 44 100 input frames in chunks
            }
            let expected = 44_100.0 * to as f64 / from as f64;
            let got = (out.len() / 2) as f64;
            assert!((got - expected).abs() <= 2.0, "{from}->{to}: {got} vs {expected}");
            // DC level preserved once past the initial ramp from prev = 0.
            assert!(out[8..].iter().all(|&s| (s - 0.25).abs() < 1e-6));
        }
    }

    #[test]
    fn mixer_sums_pads_and_caps() {
        let mut m = Mixer::new(2, 4);
        m.push(0, &[0.5, 0.5, 0.5, 0.5]);
        m.push(1, &[0.25, 0.25]);
        assert_eq!(m.pull(3), [0.75, 0.75, 0.5, 0.5, 0.0, 0.0]);
        m.push(0, &[0.9; 20]); // 10 frames, capped to 4
        assert_eq!(m.buffered(0), 4);
        m.push(1, &[0.9; 4]);
        assert_eq!(m.pull(1), [1.0, 1.0]); // clamped
    }

    #[test]
    fn levels() {
        assert_eq!(level(&[]), Level::default());
        let l = level(&sine(48_000, 0.1));
        assert!((l.rms - 0.5 / 2f32.sqrt()).abs() < 0.01, "{l:?}");
        assert!((l.peak - 0.5).abs() < 0.01);
        assert_eq!(to_i16(1.5), 32767);
        assert_eq!(to_i16(-1.0), -32767);
    }

    fn write_wav(path: &Path, frames: u32) {
        let spec = hound::WavSpec { channels: 2, sample_rate: 8_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..frames {
            w.write_sample(i as i16).unwrap();
            w.write_sample(-(i as i16)).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn trim_range_math() {
        assert_eq!(trim_range(80_000, 8_000, 1.0, 2.5), 8_000..20_000);
        assert_eq!(trim_range(80_000, 8_000, -1.0, 100.0), 0..80_000);
        assert_eq!(trim_range(80_000, 8_000, 3.0, 2.0), 24_000..24_000);
    }

    #[test]
    fn trim_wav_keeps_window() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.wav");
        write_wav(&src, 16_000); // 2 s
        let out = trim_wav(&src, 0.5, 1.25).unwrap();
        assert_eq!(out.file_name().unwrap(), "a (trim).wav");
        let mut r = hound::WavReader::open(&out).unwrap();
        assert_eq!(r.spec().sample_rate, 8_000);
        assert_eq!(r.duration(), 6_000);
        let s: Vec<i16> = r.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(&s[..2], &[4_000, -4_000]);
        assert_eq!(s[s.len() - 2], 9_999);
    }

    #[test]
    fn flac_encodes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.wav");
        write_wav(&src, 10_000);
        let flac = dir.path().join("a.flac");
        wav_to_flac(&src, &flac).unwrap();
        let bytes = std::fs::read(&flac).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        assert!(bytes.len() < std::fs::metadata(&src).unwrap().len() as usize);
    }

    /// Real devices, silent and private: ~1 s of loopback (and mic if present) into a temp
    /// dir; only the header/length is checked; the file is deleted. Nothing is played.
    #[test]
    fn record_one_second_to_temp() {
        let dir = tempfile::tempdir().unwrap();
        let h = AudioHandle::default();
        h.0.lock().unwrap().enabled = true;
        h.set_folder(dir.path().to_path_buf());
        let host = cpal::default_host();
        let src = Source { system: host.default_output_device().is_some(), mic: host.default_input_device().is_some() };
        if !src.mic && !src.system {
            eprintln!("no audio device, skipping");
            return;
        }
        let path = match h.start(src, Format::Wav) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("audio devices unavailable ({e}), skipping");
                return;
            }
        };
        std::thread::sleep(Duration::from_millis(600));
        h.pause();
        let paused_at = h.state().elapsed;
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(h.state().elapsed, paused_at);
        assert!(h.state().paused);
        h.resume();
        std::thread::sleep(Duration::from_millis(400));
        let out = h.stop().unwrap();
        assert_eq!(out, path);
        let r = hound::WavReader::open(&out).unwrap();
        assert_eq!(r.spec().sample_rate, RATE);
        assert_eq!(r.spec().channels, 2);
        let secs = r.duration() as f64 / RATE as f64;
        assert!((0.8..1.3).contains(&secs), "duration {secs}");
        drop(r);
        std::fs::remove_file(&out).unwrap();
        assert!(!h.state().recording);
    }
}
