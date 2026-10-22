//! Speech to text for subtitles: whisper.cpp through `whisper-rs`, with word times
//! from its DTW alignment.
//!
//! The model comes from the whisper.cpp repository on Hugging Face, pinned to a
//! commit and checked by SHA-256.

use std::ffi::CStr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const REPO: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1";

/// large-v3-turbo, 8 bit. Checked 2026-10-20 on a minute of Russian and English
/// stream speech, 6 CPU cores: 39-50 s with beam search. large-v3 (5 bit) took 90 s
/// and dropped a phrase; greedy decoding was no faster and lost words.
pub const MODEL_FILE: &str = "ggml-large-v3-turbo-q8_0.bin";
pub const MODEL_SHA256: &str = "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1";
/// Bytes to download.
pub const MODEL_SIZE: u64 = 874_188_075;

pub fn model_url() -> String {
    format!("{REPO}/{MODEL_FILE}")
}

/// A spoken word, seconds from the start of the audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// A discrete GPU whisper can run on (Vulkan): its number for `transcribe` and its
/// name. Integrated GPUs are left out, they are hardly faster than the CPU
/// (hypothesis). Asks Vulkan for its devices, so call it off the main thread.
pub fn gpu() -> Option<(i32, String)> {
    use whisper_rs_sys as sys;
    // whisper numbers integrated and discrete GPUs together.
    let mut n = 0;
    for i in 0..unsafe { sys::ggml_backend_dev_count() } {
        let device = unsafe { sys::ggml_backend_dev_get(i) };
        match unsafe { sys::ggml_backend_dev_type(device) } {
            sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU => {
                let name = unsafe { CStr::from_ptr(sys::ggml_backend_dev_description(device)) };
                return Some((n, name.to_string_lossy().into_owned()));
            }
            sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU => n += 1,
            _ => {}
        }
    }
    None
}

/// Words of 16 kHz mono `samples`, on GPU number `gpu` (see `gpu`) or the CPU.
/// `language` is a code like "ru"; `None` detects it. `on_progress` gets percents;
/// setting `cancel` stops early with an error.
pub fn transcribe(
    path: &Path,
    samples: &[f32],
    language: Option<&str>,
    gpu: Option<i32>,
    on_progress: impl FnMut(i32) + 'static,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<Word>, String> {
    let mut context_params = WhisperContextParameters::default();
    context_params.use_gpu(gpu.is_some()).gpu_device(gpu.unwrap_or(0));
    // Not flash attention: whisper turns DTW off with it.
    context_params.dtw_parameters(DtwParameters { mode: DtwMode::ModelPreset { model_preset: DtwModelPreset::LargeV3Turbo }, ..Default::default() });
    let path = path.to_str().ok_or("model path is not UTF-8")?;
    let context = WhisperContext::new_with_params(path, context_params).map_err(|e| e.to_string())?;
    let mut state = context.create_state().map_err(|e| e.to_string())?;

    let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
    params.set_language(Some(language.unwrap_or("auto")));
    params.set_n_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(16)) as i32);
    params.set_token_timestamps(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_progress_callback_safe(on_progress);
    params.set_abort_callback_safe(move || cancel.load(Ordering::Relaxed));
    state.full(params, samples).map_err(|e| e.to_string())?;

    let eot = context.token_eot();
    let mut tokens = Vec::new();
    for segment in state.as_iter() {
        for i in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(i) else { continue };
            let data = token.token_data();
            // Special tokens (timestamps, language, end of text) come after the text ones.
            if data.id >= eot {
                continue;
            }
            let start = if data.t_dtw >= 0 { data.t_dtw } else { data.t0 };
            tokens.push(Token { bytes: token.to_bytes().map_err(|e| e.to_string())?.to_vec(), start });
        }
    }
    Ok(words(&tokens))
}

/// A text token; `start` in centiseconds, as whisper gives it. Token end times
/// are mostly start + 10 ms (checked 2026-10-20), so words end by `spoken_end`.
struct Token {
    bytes: Vec<u8>,
    start: i64,
}

/// Joins tokens into words: a token starting with a space starts a word. Bytes are
/// joined before decoding, as a Cyrillic letter can be split between tokens.
fn words(tokens: &[Token]) -> Vec<Word> {
    let mut joined: Vec<(Vec<u8>, i64)> = Vec::new();
    for t in tokens {
        match joined.last_mut() {
            Some(word) if !t.bytes.starts_with(b" ") => word.0.extend(&t.bytes),
            _ => joined.push((t.bytes.clone(), t.start)),
        }
    }
    let words: Vec<(String, f64)> = joined
        .into_iter()
        .map(|(bytes, start)| (String::from_utf8_lossy(&bytes).trim().to_owned(), start as f64 / 100.0))
        .filter(|(text, _)| !text.is_empty())
        .collect();
    (0..words.len())
        .map(|i| {
            let (text, start) = &words[i];
            let end = spoken_end(*start, words.get(i + 1).map(|w| w.1), text.chars().count());
            Word { start: *start, end, text: text.clone() }
        })
        .collect()
}

/// When a word stops sounding: when the next starts, but no later than its length
/// suggests, so a word before a pause does not hang on.
fn spoken_end(start: f64, next: Option<f64>, chars: usize) -> f64 {
    let guess = start + (0.2 + 0.07 * chars as f64).min(1.2);
    let next = next.unwrap_or(f64::INFINITY);
    // At least 50 ms, unless the next word starts sooner.
    guess.min(next).max((start + 0.05).min(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &[u8], start: i64) -> Token {
        Token { bytes: text.to_vec(), start }
    }

    #[test]
    fn tokens_join_into_words() {
        // "Привет" split inside a letter, then a comma glued to it.
        let privet = "Привет".as_bytes();
        let tokens = [
            token(b" ", 0),
            token(&[b" ".as_slice(), &privet[..5]].concat(), 10),
            token(&privet[5..], 30),
            token(b",", 60),
            token(b" world", 55),
            token(b"!", 90),
            token(b" again", 400),
        ];
        let words = words(&tokens);
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Привет,", "world!", "again"]);
        assert_eq!((words[0].start, words[0].end), (0.1, 0.55));
        // A pause follows: the length of the word decides.
        assert!((words[1].end - (0.55 + 0.2 + 0.07 * 6.0)).abs() < 1e-9);
        assert!((words[2].end - (4.0 + 0.2 + 0.07 * 5.0)).abs() < 1e-9);
    }

    #[test]
    fn word_ends_before_the_next() {
        // "я" at 28.18 and the next word 40 ms later (seen on a real VOD).
        assert_eq!(spoken_end(28.18, Some(28.22), 1), 28.22);
        assert!((spoken_end(1.0, Some(1.0), 1) - 1.0).abs() < 1e-9);
        assert!((spoken_end(1.0, None, 0) - 1.2).abs() < 1e-9);
    }

    /// Words of a minute of a VOD; prints them with times and the speed, on the GPU
    /// if there is one (`WCLIP_CPU=1`: on the CPU):
    /// `WCLIP_MODEL=<file> [WCLIP_VOD=<id>] [WCLIP_AT=<s>] [WCLIP_LANG=ru] [WCLIP_CPU=1]
    /// cargo test --release -- --ignored smoke_transcribe --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_transcribe() {
        use crate::tools::{self, Tools};
        let var = |k: &str| std::env::var(k).ok();
        let path = std::path::PathBuf::from(var("WCLIP_MODEL").expect("WCLIP_MODEL"));
        let vod_id = var("WCLIP_VOD").unwrap_or_else(|| "2905205256".into());
        let at: f64 = var("WCLIP_AT").map_or(600.0, |v| v.parse().unwrap());
        let tools = Tools::from_status(&tools::status(&std::env::temp_dir())).expect("tools in PATH");
        let vod = crate::cut::vod_info(&tools, &vod_id, None).await.unwrap();
        let samples = crate::cut::audio(&tools, vod.audio_input(), at, 60.0).await.unwrap();
        assert!((samples.len() as f64 / 16000.0 - 60.0).abs() < 0.5, "{} samples", samples.len());
        let gpu = if var("WCLIP_CPU").is_some() { None } else { gpu() };
        println!("on {}", gpu.as_ref().map_or("the CPU", |g| &g.1));
        let started = std::time::Instant::now();
        let lang = var("WCLIP_LANG");
        let words = tokio::task::spawn_blocking(move || {
            transcribe(&path, &samples, lang.as_deref(), gpu.map(|g| g.0), |_| {}, Arc::new(AtomicBool::new(false)))
        })
        .await
        .unwrap()
        .unwrap();
        println!("60 s in {:.1} s, {} words", started.elapsed().as_secs_f64(), words.len());
        for w in &words {
            println!("{:>6.2}-{:<6.2} {}", w.start, w.end, w.text);
        }
        assert!(words.len() > 20);
        assert!(words.windows(2).all(|w| w[0].start <= w[1].start && w[0].end <= w[1].start + 1e-9));
    }

    /// A GPU number that does not exist must come back as an error (which `spoken_words`
    /// retries on the CPU), not abort the process:
    /// `WCLIP_MODEL=<file> cargo test --release -- --ignored smoke_missing_gpu --nocapture`.
    #[test]
    #[ignore]
    fn smoke_missing_gpu() {
        let path = std::path::PathBuf::from(std::env::var("WCLIP_MODEL").expect("WCLIP_MODEL"));
        let samples = vec![0.0f32; 16000 * 5];
        let result = transcribe(&path, &samples, Some("en"), Some(99), |_| {}, Arc::new(AtomicBool::new(false)));
        println!("device 99: {:?}", result.as_ref().map(|w| w.len()));
    }
}
