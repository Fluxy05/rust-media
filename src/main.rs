use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::audio::{AudioCallback, AudioSpecDesired};
use std::time::{Duration, Instant};
use std::env;
use ffmpeg::software::scaling::{context::Context as ScalingContext, flag::Flags};
use ffmpeg::util::frame::video::Video;
use ffmpeg::format::Pixel;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const AUDIO_BUFFER_SIZE: usize = 16384;
const AUDIO_SAMPLE_RATE: i32 = 44100;
const AUDIO_CHANNELS: u8 = 2;
const AUDIO_SYNC_THRESHOLD: f64 = 0.1;
const AUDIO_BUFFER_MIN_SIZE: usize = 8192;
const VIDEO_SYNC_THRESHOLD: Duration = Duration::from_millis(5);
const TARGET_FPS: f64 = 60.0;
const SYNC_THRESHOLD: Duration = Duration::from_millis(2);

struct AudioState {
    current_time: f64,
}

struct AudioPlayer {
    buffer: VecDeque<f32>,
    channels: u8,
    time_base: f64,
    state: Arc<Mutex<AudioState>>,
    sample_rate: i32,
}

impl AudioPlayer {
    fn new(channels: u8, time_base: f64, sample_rate: i32) -> Self {
        Self {
            buffer: VecDeque::with_capacity(AUDIO_BUFFER_SIZE * channels as usize),
            channels,
            time_base,
            state: Arc::new(Mutex::new(AudioState { current_time: 0.0 })),
            sample_rate,
        }
    }

    fn add_samples(&mut self, samples: &[f32], pts: i64) {
        let current_time = pts as f64 * self.time_base;
        if let Ok(mut state) = self.state.lock() {
            state.current_time = current_time;
        }

        // Gestion du buffer avec contrôle de dépassement
        let buffer_space = AUDIO_BUFFER_SIZE * self.channels as usize - self.buffer.len();
        let samples_to_add = samples.len().min(buffer_space);

        // Ajouter les échantillons au buffer
        for &sample in samples.iter().take(samples_to_add) {
            if self.buffer.len() < AUDIO_BUFFER_SIZE * self.channels as usize {
                self.buffer.push_back(sample);
            }
        }
    }

    fn get_state(&self) -> Arc<Mutex<AudioState>> {
        self.state.clone()
    }
}

impl AudioCallback for AudioPlayer {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        for sample in out.iter_mut() {
            if self.buffer.is_empty() {
                *sample = 0.0;
            } else {
                *sample = self.buffer.pop_front().unwrap();
            }
        }
    }
}

struct Decoder {
    decoder: ffmpeg::codec::decoder::Video,
    scaler: ScalingContext,
    time_base: f64,
    frame_rate: f64,
    start_time: Option<Instant>,
    frame_duration: Duration,
    frame_count: u64,
    last_frame_time: Option<Instant>,
    next_frame_target: Option<Instant>,
    total_drift: Duration,
}

impl Decoder {
    fn new(decoder: ffmpeg::codec::decoder::Video, stream: &ffmpeg::Stream) -> Result<Self> {
        let time_base = f64::from(stream.time_base());
        let frame_rate = f64::from(stream.rate());
        let frame_duration = Duration::from_secs_f64(1.0 / frame_rate);

        println!("Initialisation décodeur vidéo:");
        println!("  Time base: {}", time_base);
        println!("  Frame rate: {} fps", frame_rate);
        println!("  Frame duration: {:?}", frame_duration);

        let scaler = ScalingContext::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::YUV420P,
            decoder.width(),
            decoder.height(),
            Flags::BILINEAR,
        )?;

        Ok(Self {
            decoder,
            scaler,
            time_base,
            frame_rate,
            start_time: None,
            frame_duration,
            frame_count: 0,
            last_frame_time: None,
            next_frame_target: None,
            total_drift: Duration::ZERO,
        })
    }

    fn receive_frame_yuv(&mut self, frame: &mut Video) -> Result<bool> {
        match self.decoder.receive_frame(frame) {
            Ok(_) => {
                let mut yuv_frame = Video::empty();
                self.scaler.run(frame, &mut yuv_frame)?;
                frame.clone_from(&yuv_frame);
                Ok(true)
            }
            Err(ffmpeg::Error::Other { errno: ffmpeg::error::EAGAIN }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn should_display_frame(&mut self, pts: i64) -> bool {
        let now = Instant::now();

        if self.start_time.is_none() {
            self.start_time = Some(now);
            self.last_frame_time = Some(now);
            self.next_frame_target = Some(now + self.frame_duration);
            println!("Première frame - Démarrage à {:?}", now);
            return true;
        }

        // Calculer le temps vidéo en utilisant le time_base (1/16000)
        let video_time = Duration::from_secs_f64(pts as f64 * self.time_base);
        let elapsed = self.start_time.unwrap().elapsed();

        // Vérifier si nous avons atteint le temps cible pour la prochaine frame
        let target_time = self.next_frame_target.unwrap();
        if now < target_time {
            // Trop tôt pour afficher la frame suivante
            std::thread::sleep(target_time.duration_since(now));
            return false;
        }

        // Calculer l'intervalle depuis la dernière frame
        let frame_interval = if let Some(last) = self.last_frame_time {
            now.duration_since(last)
        } else {
            Duration::ZERO
        };

        // Mettre à jour les compteurs
        self.frame_count += 1;
        self.last_frame_time = Some(now);
        self.next_frame_target = Some(target_time + self.frame_duration);

        // Log toutes les 30 frames
        if self.frame_count % 30 == 0 {
            let current_fps = 1.0 / frame_interval.as_secs_f64();
            println!("Frame {} - Stats:", self.frame_count);
            println!("  Intervalle: {:.2}ms", frame_interval.as_secs_f64() * 1000.0);
            println!("  FPS actuel: {:.2}", current_fps);
            println!("  Temps vidéo: {:.2}ms", video_time.as_secs_f64() * 1000.0);
            println!("  Temps réel: {:.2}ms", elapsed.as_secs_f64() * 1000.0);
            println!("  PTS: {}", pts);

            if elapsed > video_time {
                println!("  Retard: {:.2}ms", (elapsed - video_time).as_secs_f64() * 1000.0);
            } else {
                println!("  Avance: {:.2}ms", (video_time - elapsed).as_secs_f64() * 1000.0);
            }
        }

        true
    }
}

fn init_ffmpeg() -> Result<()> {
    ffmpeg::init()?;
    Ok(())
}

fn open_decoders(path: &str) -> Result<(ffmpeg::format::context::Input, Decoder, Option<ffmpeg::codec::decoder::Audio>)> {
    let ictx = ffmpeg::format::input(&path)?;

    let video_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("Aucun flux vidéo trouvé")?;

    println!("Information flux vidéo:");
    println!("  Time base: {}", video_stream.time_base());
    println!("  Frame rate: {}", video_stream.rate());
    println!("  Duration: {} secondes", video_stream.duration() as f64 * f64::from(video_stream.time_base()));

    let context = ffmpeg::codec::Context::from_parameters(video_stream.parameters())?;
    let codec_id = context.id();
    println!("  Codec: {:?}", codec_id);

    // Liste des décodeurs matériels pour H.264 et H.265
    let hw_decoders = match codec_id {
        ffmpeg::codec::id::Id::H264 => vec!["h264_nvdec", "h264_vaapi", "h264_qsv"],
        ffmpeg::codec::id::Id::HEVC => vec!["hevc_nvdec", "hevc_vaapi", "hevc_qsv"],
        ffmpeg::codec::id::Id::AV1 => vec!["av1_nvdec", "av1_vaapi", "av1_qsv"],
        _ => vec![],
    };

    let mut found_hw_decoder = false;
    let mut decoder_name = "";

    for &name in hw_decoders.iter() {
        if let Some(_) = ffmpeg::codec::decoder::find_by_name(name) {
            println!("Décodeur matériel trouvé: {}", name);
            found_hw_decoder = true;
            decoder_name = name;
            break;
        }
    }

    if !found_hw_decoder {
        println!("Aucun décodeur matériel disponible, utilisation du décodage logiciel");
    }

    let video_decoder = context.decoder().video()?;
    let decoder = Decoder::new(video_decoder, &video_stream)?;

    let audio_decoder = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .and_then(|stream| {
            println!("Information flux audio:");
            println!("  Time base: {}", stream.time_base());
            let context = ffmpeg::codec::Context::from_parameters(stream.parameters()).ok()?;
            let audio_dec = context.decoder().audio().ok()?;
            let sample_rate = audio_dec.rate() as i32;
            println!("  Channels: {}", audio_dec.channels());
            println!("  Sample format: {:?}", audio_dec.format());
            println!("  Sample rate: {} Hz", sample_rate);
            Some((audio_dec, sample_rate))
        });

    Ok((ictx, decoder, audio_decoder.map(|(dec, _)| dec)))
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <chemin_video>", args[0]);
        std::process::exit(1);
    }
    let video_path = &args[1];

    init_ffmpeg()?;

    let (mut ictx, mut decoder, mut audio_decoder) = open_decoders(video_path)?;
    let video_stream_index = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("Aucun flux vidéo trouvé")?
        .index();

    let audio_stream_index = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|stream| stream.index());

    let sdl_context = sdl2::init().map_err(|e| anyhow::anyhow!(e))?;
    let video_subsystem = sdl_context.video().map_err(|e| anyhow::anyhow!(e))?;
    let audio_subsystem = sdl_context.audio().map_err(|e| anyhow::anyhow!(e))?;

    let mut audio_device = if let Some(ref audio_dec) = audio_decoder {
        let channels = audio_dec.channels() as u8;
        let audio_stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .context("No audio stream found")?;
        let audio_time_base = f64::from(audio_stream.time_base());
        let sample_rate = audio_dec.rate() as i32;

        println!("Configuration audio:");
        println!("  Channels: {}", channels);
        println!("  Sample rate: {} Hz", sample_rate);
        println!("  Buffer size: {}", AUDIO_BUFFER_SIZE);

        let desired_spec = AudioSpecDesired {
            freq: Some(sample_rate),
            channels: Some(channels),
            samples: Some(4096),
        };

        let audio_player = AudioPlayer::new(channels, audio_time_base, sample_rate);
        let audio_state = audio_player.get_state();
        let device = audio_subsystem.open_playback(None, &desired_spec, |_| audio_player)
            .map_err(|e| anyhow::anyhow!(e))?;
        Some((device, audio_state))
    } else {
        None
    };

    let window = video_subsystem
        .window("Lecteur Vidéo Rust", decoder.decoder.width() as u32, decoder.decoder.height() as u32)
        .position_centered()
        .build()
        .map_err(|e| anyhow::anyhow!(e))?;

    let mut canvas = window.into_canvas()
        .build()
        .map_err(|e| anyhow::anyhow!(e))?;

    canvas.set_draw_color(sdl2::pixels::Color::BLACK);
    canvas.clear();
    canvas.present();

    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(
            PixelFormatEnum::IYUV,
            decoder.decoder.width() as u32,
            decoder.decoder.height() as u32
        )
        .map_err(|e| anyhow::anyhow!(e))?;

    let mut event_pump = sdl_context.event_pump().map_err(|e| anyhow::anyhow!(e))?;

    let mut frame = Video::empty();
    let mut audio_frame = ffmpeg::frame::Audio::empty();

    if let Some((ref device, _)) = audio_device {
        device.resume();
    }

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } |
                Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                    break 'running;
                }
                _ => {}
            }
        }

        match ictx.packets().next() {
            Some((stream, packet)) => {
                if stream.index() == video_stream_index {
                    let packet_pts = packet.pts().unwrap_or(0);
                    decoder.decoder.send_packet(&packet)?;

                    if decoder.receive_frame_yuv(&mut frame)? {
                        if decoder.should_display_frame(packet_pts) {
                            texture.update_yuv(
                                None,
                                frame.data(0),
                                frame.stride(0),
                                frame.data(1),
                                frame.stride(1),
                                frame.data(2),
                                frame.stride(2)
                            ).map_err(|e| anyhow::anyhow!(e))?;

                            canvas.clear();
                            canvas.copy(&texture, None, None)
                                .map_err(|e| anyhow::anyhow!(e))?;
                            canvas.present();
                        }
                    }
                } else if Some(stream.index()) == audio_stream_index {
                    if let Some(ref mut audio_dec) = audio_decoder {
                        audio_dec.send_packet(&packet)?;

                        while audio_dec.receive_frame(&mut audio_frame).is_ok() {
                            if let Some((ref mut device, _)) = audio_device {
                                let mut audio_player = device.lock();
                                let samples = audio_frame.plane::<f32>(0);
                                let pts = packet.pts().unwrap_or(0);
                                audio_player.add_samples(samples, pts);
                            }
                        }
                    }
                }
            }
            None => break,
        }
    }

    Ok(())
}
