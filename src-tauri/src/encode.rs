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
        // level (CRF, CQ or QP: lower is better). A level means the same picture on
        // every encoder: VMAF against the Twitch source as x264's (95 / 93 / 89),
        // measured 2026-10-21 on four 1080p60 VOD moments (Plan.md, stage 18).
        // NVENC, QSV and AMF are not measured: their levels are from the docs.
        let (name, options, quality_flags, levels): (&str, &[&str], &[&str], [u8; 3]) = match self.encoder {
            Encoder::Auto | Encoder::Cpu if hevc => ("libx265", &["-preset", "fast"], &["-crf"], [22, 26, 30]),
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
            // Constant QP without a bitrate cap: QP 20 made busy scenes 2.6 times the
            // source bitrate. Mesa has no QVBR, and VBR at the source bitrate looks
            // worse (VMAF 90).
            Encoder::Vaapi => (pick("h264_vaapi", "hevc_vaapi"), &["-rc_mode", "CQP"], &["-qp"], [24, 28, 32]),
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
            // Apple players and some editors refuse HEVC in MP4 tagged `hev1`. With
            // `hvc1` the muxer drops the parameter sets from the stream and keeps the
            // header ffmpeg wrote before encoding, but Mesa's VAAPI writes its own
            // SPS/PPS (other QP, dependent slices): the clip decodes green and broken.
            // The filter takes the header from the stream instead.
            codec.extend(["-tag:v", "hvc1", "-bsf:v", "extract_extradata"].map(str::to_owned));
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
        assert!(a.codec.ends_with(&["-tag:v", "hvc1", "-bsf:v", "extract_extradata"].map(str::to_owned)));

        let v = Output { encoder: Encoder::Vaapi, ..Output::default() }.args();
        assert_eq!(v.global[0], "-vaapi_device");
        assert_eq!(v.filter, "format=nv12,hwupload");
        assert_eq!(v.codec[1], "h264_vaapi");
        assert!(v.codec.windows(2).any(|w| w == ["-qp", "24"]), "{:?}", v.codec);
    }

    /// "High" on every working encoder looks like x264's, VMAF against 10 s of a busy
    /// VOD scene (where an uncapped encoder overshoots most); ffmpeg with libvmaf
    /// and yt-dlp from `PATH`: `cargo test -- --ignored smoke_quality --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn smoke_quality() {
        let tools = Tools::from_status(&tools::status(&std::env::temp_dir())).expect("tools in PATH");
        let dir = std::env::temp_dir().join(format!("wclip-test-quality-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let vod = crate::cut::vod_info(&tools, "2905205256", None).await.unwrap();
        let source = dir.join("source.mp4");
        let ffmpeg = || std::process::Command::new(&tools.ffmpeg);
        let ok = ffmpeg()
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "20000", "-i", &vod.playlist, "-t", "10", "-an", "-c", "copy"])
            .arg(&source)
            .status()
            .unwrap();
        assert!(ok.success());
        for codec in [Codec::H264, Codec::Hevc] {
            for encoder in std::iter::once(Encoder::Cpu).chain(available(&tools, codec).await) {
                let args = Output { encoder, codec, quality: Quality::High }.args();
                let out = dir.join(format!("{encoder:?}_{codec:?}.mp4"));
                let ok = ffmpeg()
                    .args(["-hide_banner", "-loglevel", "error", "-y"])
                    .args(&args.global)
                    .arg("-i")
                    .arg(&source)
                    .args(["-vf", &args.filter])
                    .args(&args.codec)
                    .arg(&out)
                    .status()
                    .unwrap();
                assert!(ok.success());
                let vmaf = ffmpeg()
                    .args(["-hide_banner", "-i"])
                    .arg(&out)
                    .arg("-i")
                    .arg(&source)
                    .args(["-lavfi", "[0:v]setpts=PTS-STARTPTS[d];[1:v]setpts=PTS-STARTPTS[r];[d][r]libvmaf=n_subsample=3"])
                    .args(["-f", "null", "-"])
                    .output()
                    .unwrap();
                let log = String::from_utf8_lossy(&vmaf.stderr);
                let score: f64 = log.split("VMAF score: ").nth(1).and_then(|s| s.split_whitespace().next()).unwrap().parse().unwrap();
                let size = std::fs::metadata(&out).unwrap().len() as f64 * 8.0 / 10.0 / 1e6;
                println!("{encoder:?} {codec:?}: VMAF {score:.1}, {size:.1} Mbit/s");
                assert!((94.3..=96.3).contains(&score), "{encoder:?} {codec:?}: VMAF {score:.1}");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
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

    /// MP4 from every working encoder decodes close to its source (Mesa's VAAPI HEVC
    /// with `hvc1` used to come out green): `cargo test -- --ignored smoke_mp4_decodes --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn smoke_mp4_decodes() {
        let tools = Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), yt_dlp: "yt-dlp".into() };
        let dir = std::env::temp_dir().join(format!("wclip-test-encoders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = "testsrc2=s=1080x1920:r=60:d=2";
        for codec in [Codec::H264, Codec::Hevc] {
            for encoder in std::iter::once(Encoder::Cpu).chain(available(&tools, codec).await) {
                let args = Output { encoder, codec, quality: Quality::High }.args();
                let out = dir.join(format!("{encoder:?}_{codec:?}.mp4"));
                let status = std::process::Command::new("ffmpeg")
                    .args(["-hide_banner", "-loglevel", "error", "-y"])
                    .args(&args.global)
                    .args(["-f", "lavfi", "-i", source, "-vf", &args.filter])
                    .args(&args.codec)
                    .arg(&out)
                    .status()
                    .unwrap();
                assert!(status.success());
                let psnr = std::process::Command::new("ffmpeg")
                    .args(["-hide_banner", "-i"])
                    .arg(&out)
                    .args(["-f", "lavfi", "-i", source])
                    .args(["-filter_complex", "[0:v]format=yuv420p[a];[1:v]format=yuv420p[b];[a][b]psnr", "-f", "null", "-"])
                    .output()
                    .unwrap();
                let log = String::from_utf8_lossy(&psnr.stderr);
                let average: f64 = log.split("average:").nth(1).and_then(|s| s.split_whitespace().next()).unwrap().parse().unwrap();
                println!("{encoder:?} {codec:?}: PSNR {average:.1} dB");
                assert!(average > 35.0, "{encoder:?} {codec:?} decodes broken");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
