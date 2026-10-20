//! Video encoder choice: libx264/libx265 or a GPU encoder, found by trying each
//! with a tiny ffmpeg run (an encoder listed by `ffmpeg -encoders` may still have
//! no device or driver behind it).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::tools::{self, Tools};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoder {
    /// The first GPU encoder that works, else the CPU.
    #[default]
    Auto,
    /// libx264 / libx265.
    Cpu,
    Nvenc,
    Qsv,
    Amf,
    Vaapi,
}

/// GPU encoders in the order `Auto` tries them.
pub const GPU: [Encoder; 4] = [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Vaapi];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    H264,
    Hevc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Visually lossless; what v0.1 always used (x264 CRF 18).
    #[default]
    High,
    Medium,
    /// For sharing as is: about a third of `High`.
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    pub encoder: Encoder,
    pub codec: Codec,
    pub quality: Quality,
}

/// How to encode the video stream, once `Encoder::Auto` is resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoArgs {
    /// Before the inputs.
    pub global: Vec<String>,
    /// Last filter of the video chain: pixel format, and upload for VAAPI.
    pub filter: String,
    pub codec: Vec<String>,
}

impl Output {
    /// Same settings on the CPU: the fallback when a GPU encoder fails.
    pub fn on_cpu(self) -> Self {
        Self { encoder: Encoder::Cpu, ..self }
    }

    /// ffmpeg arguments; `encoder` must not be `Auto`.
    pub fn args(self) -> VideoArgs {
        let hevc = self.codec == Codec::Hevc;
        let pick = |h264: &'static str, h265: &'static str| if hevc { h265 } else { h264 };
        // Encoder, its options, the flags taking the quality and the quality per
        // level (CRF, CQ or QP: lower is better).
        let (name, options, quality_flags, levels): (&str, &[&str], &[&str], [u8; 3]) = match self.encoder {
            Encoder::Auto | Encoder::Cpu if hevc => ("libx265", &["-preset", "fast"], &["-crf"], [20, 24, 28]),
            Encoder::Auto | Encoder::Cpu => ("libx264", &["-preset", "veryfast"], &["-crf"], [18, 22, 26]),
            Encoder::Nvenc => (
                pick("h264_nvenc", "hevc_nvenc"),
                &["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-b:v", "0"],
                &["-cq"],
                [19, 24, 28],
            ),
            Encoder::Qsv => (pick("h264_qsv", "hevc_qsv"), &["-preset", "faster"], &["-global_quality"], [20, 24, 28]),
            Encoder::Amf => {
                (pick("h264_amf", "hevc_amf"), &["-quality", "balanced", "-rc", "cqp"], &["-qp_i", "-qp_p"], [20, 24, 28])
            }
            Encoder::Vaapi => (pick("h264_vaapi", "hevc_vaapi"), &["-rc_mode", "CQP"], &["-qp"], [20, 24, 28]),
        };
        let level = match self.quality {
            Quality::High => levels[0],
            Quality::Medium => levels[1],
            Quality::Small => levels[2],
        };
        let mut codec = vec!["-c:v".to_owned(), name.to_owned()];
        codec.extend(options.iter().map(|&o| o.to_owned()));
        for &flag in quality_flags {
            codec.extend([flag.to_owned(), level.to_string()]);
        }
        if hevc {
            // Apple players and some editors refuse HEVC in MP4 tagged `hev1`.
            codec.extend(["-tag:v".to_owned(), "hvc1".to_owned()]);
        }
        let (global, filter) = match self.encoder {
            Encoder::Vaapi => (vec!["-vaapi_device".to_owned(), vaapi_device().to_string_lossy().into_owned()], "format=nv12,hwupload"),
            _ => (Vec::new(), "format=yuv420p"),
        };
        VideoArgs { global, filter: filter.to_owned(), codec }
    }
}

/// First DRM render node; VAAPI encodes there.
fn vaapi_device() -> PathBuf {
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD")))
        .collect();
    nodes.sort();
    nodes.into_iter().next().unwrap_or_else(|| Path::new("/dev/dri/renderD128").to_owned())
}

/// Whether `encoder` can encode `codec` here: a 0.2 s test video through it.
async fn works(tools: &Tools, output: Output) -> bool {
    let args = output.args();
    let status = tokio::process::Command::from(tools::command(&tools.ffmpeg))
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .args(&args.global)
        .args(["-f", "lavfi", "-i", "color=black:s=1280x720:r=30:d=0.2"])
        .args(["-vf", &args.filter])
        .args(&args.codec)
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await;
    status.is_ok_and(|s| s.success())
}

/// GPU encoders that work for `codec` here, in `GPU` order.
pub async fn available(tools: &Tools, codec: Codec) -> Vec<Encoder> {
    let mut out = Vec::new();
    for encoder in GPU {
        if works(tools, Output { encoder, codec, quality: Quality::High }).await {
            out.push(encoder);
        }
    }
    out
}

/// `output` with `Auto` replaced by the first of `gpus` (see `available`), or the CPU.
pub fn resolve(output: Output, gpus: &[Encoder]) -> Output {
    match output.encoder {
        Encoder::Auto => Output { encoder: gpus.first().copied().unwrap_or(Encoder::Cpu), ..output },
        _ => output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_default_matches_v0_1() {
        let args = Output::default().on_cpu().args();
        assert_eq!(args.codec, ["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"]);
        assert_eq!(args.filter, "format=yuv420p");
        assert!(args.global.is_empty());
    }

    #[test]
    fn gpu_args() {
        let o = Output { encoder: Encoder::Nvenc, codec: Codec::Hevc, quality: Quality::Small };
        let a = o.args();
        assert_eq!(a.codec[1], "hevc_nvenc");
        assert!(a.codec.windows(2).any(|w| w == ["-cq", "28"]));
        assert!(a.codec.ends_with(&["-tag:v".to_owned(), "hvc1".to_owned()]));

        let v = Output { encoder: Encoder::Vaapi, ..Output::default() }.args();
        assert_eq!(v.global[0], "-vaapi_device");
        assert_eq!(v.filter, "format=nv12,hwupload");
        assert_eq!(v.codec[1], "h264_vaapi");
    }

    #[test]
    fn resolves_auto() {
        let auto = Output::default();
        assert_eq!(resolve(auto, &[]).encoder, Encoder::Cpu);
        assert_eq!(resolve(auto, &[Encoder::Vaapi]).encoder, Encoder::Vaapi);
        let fixed = Output { encoder: Encoder::Cpu, ..auto };
        assert_eq!(resolve(fixed, &[Encoder::Nvenc]).encoder, Encoder::Cpu);
    }

    /// Tries every GPU encoder with ffmpeg from `PATH`: `cargo test -- --ignored smoke_encoders --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn smoke_encoders() {
        let tools = Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), yt_dlp: "yt-dlp".into() };
        for codec in [Codec::H264, Codec::Hevc] {
            println!("{codec:?}: {:?}", available(&tools, codec).await);
        }
        assert!(works(&tools, Output::default().on_cpu()).await);
    }
}
