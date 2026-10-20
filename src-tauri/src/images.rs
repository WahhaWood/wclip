//! Disk cache for badge and emote images. Images decode to RGBA; animated ones
//! (GIF, WebP) also keep every frame with its timing.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frame, ImageFormat, RgbaImage};

/// Frames kept of a long animation; the rest are dropped and it loops sooner.
const MAX_FRAMES: usize = 256;
/// Browsers show frames with a shorter delay (often 0) for 100 ms; so do we.
const MIN_DELAY_MS: f64 = 20.0;
const DEFAULT_DELAY_MS: f64 = 100.0;

/// An image with more than one frame, looping forever.
#[derive(Debug, Clone)]
pub struct Animation {
    pub frames: Vec<RgbaImage>,
    /// When each frame ends within one loop, seconds; the last is the loop length.
    ends: Vec<f64>,
}

impl Animation {
    pub(crate) fn new(frames: Vec<Frame>) -> Self {
        let mut t = 0.0;
        let mut ends = Vec::with_capacity(frames.len());
        for f in &frames {
            let (n, d) = f.delay().numer_denom_ms();
            let ms = f64::from(n) / f64::from(d.max(1));
            t += if ms < MIN_DELAY_MS { DEFAULT_DELAY_MS } else { ms } / 1000.0;
            ends.push(t);
        }
        Self { frames: frames.into_iter().map(Frame::into_buffer).collect(), ends }
    }

    /// Index of the frame shown `t` seconds into the animation.
    pub fn index_at(&self, t: f64) -> usize {
        let total = self.ends.last().copied().unwrap_or(1.0);
        let t = t.rem_euclid(total);
        self.ends.partition_point(|&end| end <= t).min(self.frames.len() - 1)
    }
}

/// A decoded image: the first frame, and all of them if it moves.
#[derive(Debug, Clone)]
pub struct Picture {
    pub still: RgbaImage,
    pub animation: Option<Animation>,
}

/// Images of a chat by URL: first frames of all, frames of the animated ones.
#[derive(Debug, Clone, Default)]
pub struct Images {
    pub stills: HashMap<String, RgbaImage>,
    pub animations: HashMap<String, Animation>,
}

impl From<HashMap<String, RgbaImage>> for Images {
    fn from(stills: HashMap<String, RgbaImage>) -> Self {
        Self { stills, animations: HashMap::new() }
    }
}

fn decode(bytes: &[u8]) -> Result<Picture> {
    let take = |frames: image::Frames| frames.take(MAX_FRAMES).collect::<image::ImageResult<Vec<Frame>>>();
    let frames = match image::guess_format(bytes) {
        Ok(ImageFormat::Gif) => take(GifDecoder::new(Cursor::new(bytes))?.into_frames())?,
        Ok(ImageFormat::WebP) => {
            let decoder = WebPDecoder::new(Cursor::new(bytes))?;
            if decoder.has_animation() { take(decoder.into_frames())? } else { Vec::new() }
        }
        _ => Vec::new(),
    };
    if frames.len() > 1 {
        let animation = Animation::new(frames);
        return Ok(Picture { still: animation.frames[0].clone(), animation: Some(animation) });
    }
    Ok(Picture { still: image::load_from_memory(bytes)?.to_rgba8(), animation: None })
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("image: {0}")]
    Decode(#[from] image::ImageError),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct ImageCache {
    dir: PathBuf,
    client: reqwest::Client,
}

impl ImageCache {
    pub fn new(dir: PathBuf, client: reqwest::Client) -> Self {
        Self { dir, client }
    }

    pub async fn get(&self, url: &str) -> Result<Picture> {
        let path = self.dir.join(format!("{:016x}", crate::fnv1a(url.as_bytes())));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                let bytes = self.client.get(url).send().await?.error_for_status()?.bytes().await?;
                std::fs::create_dir_all(&self.dir)?;
                let part = path.with_extension("part");
                std::fs::write(&part, &bytes)?;
                std::fs::rename(&part, &path)?;
                bytes.to_vec()
            }
        };
        // Decoding every frame of a big animation takes a while.
        tokio::task::spawn_blocking(move || decode(&bytes)).await.map_err(std::io::Error::other)?
    }

    /// Every image that loads, a few downloads at a time.
    pub async fn get_all(&self, urls: impl IntoIterator<Item = String>) -> Images {
        const PARALLEL: usize = 16;
        let mut out = Images::default();
        let mut add = |url: String, picture: Picture| {
            if let Some(animation) = picture.animation {
                out.animations.insert(url.clone(), animation);
            }
            out.stills.insert(url, picture.still);
        };
        let mut tasks = tokio::task::JoinSet::new();
        for url in urls {
            if tasks.len() >= PARALLEL
                && let Some(Ok((url, Ok(picture)))) = tasks.join_next().await
            {
                add(url, picture);
            }
            let cache = self.clone();
            tasks.spawn(async move {
                let picture = cache.get(&url).await;
                (url, picture)
            });
        }
        while let Some(done) = tasks.join_next().await {
            if let Ok((url, Ok(picture))) = done {
                add(url, picture);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Rgba};

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    /// Puts `bytes` where the cache keeps `url`, so `get` never touches the network.
    fn cached(name: &str, url: &str, bytes: &[u8]) -> ImageCache {
        let dir = std::env::temp_dir().join(format!("wclip-test-img-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{:016x}", crate::fnv1a(url.as_bytes()))), bytes).unwrap();
        ImageCache::new(dir, reqwest::Client::new())
    }

    fn encode(format: ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        RgbaImage::from_pixel(4, 3, RED).write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[tokio::test]
    async fn decodes_png_and_webp() {
        for (name, format) in [("png", ImageFormat::Png), ("webp", ImageFormat::WebP)] {
            let cache = cached(name, "https://x/e", &encode(format));
            let picture = cache.get("https://x/e").await.unwrap();
            assert_eq!(picture.still.dimensions(), (4, 3), "{name}");
            assert_eq!(*picture.still.get_pixel(0, 0), RED, "{name}");
            assert!(picture.animation.is_none());
        }
    }

    #[tokio::test]
    async fn animated_gif_keeps_frames_and_timing() {
        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut bytes);
            for (color, ms) in [(RED, 100), (BLUE, 300)] {
                let frame = Frame::from_parts(RgbaImage::from_pixel(2, 2, color), 0, 0, Delay::from_numer_denom_ms(ms, 1));
                enc.encode_frame(frame).unwrap();
            }
        }
        let cache = cached("gif", "https://x/g", &bytes);
        let picture = cache.get("https://x/g").await.unwrap();
        assert_eq!(*picture.still.get_pixel(1, 1), RED);
        let anim = picture.animation.unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(*anim.frames[1].get_pixel(1, 1), BLUE);
        let at = |t: f64| anim.index_at(t);
        assert_eq!((at(0.0), at(0.099), at(0.1), at(0.399), at(0.4), at(0.55)), (0, 0, 1, 1, 0, 1), "loops every 0.4 s");
        assert_eq!(at(-0.05), 1, "before the start it still loops");
    }

    #[test]
    fn zero_delay_is_100_ms() {
        let frames = (0..2).map(|_| Frame::from_parts(RgbaImage::new(1, 1), 0, 0, Delay::from_numer_denom_ms(0, 1))).collect();
        let anim = Animation::new(frames);
        assert_eq!((anim.index_at(0.05), anim.index_at(0.15)), (0, 1));
    }

    /// `cargo test -- --ignored smoke_images`
    #[tokio::test]
    #[ignore]
    async fn smoke_images() {
        let dir = std::env::temp_dir().join(format!("wclip-smoke-img-{}", std::process::id()));
        let cache = ImageCache::new(dir.clone(), reqwest::Client::new());
        for url in [
            "https://static-cdn.jtvnw.net/emoticons/v2/25/static/dark/2.0",
            "https://cdn.betterttv.net/emote/54fa8f1401e468494b85b537/2x",
            "https://cdn.7tv.app/emote/01FHMGJRDR000FFDX954T14FX0/2x.webp",
        ] {
            let picture = cache.get(url).await.unwrap();
            assert!(picture.still.width() > 0, "{url}");
            // Second call is served from disk.
            assert_eq!(cache.get(url).await.unwrap().still, picture.still);
        }
        // PETPET (7TV global) and an animated Twitch emote.
        for url in [
            "https://cdn.7tv.app/emote/01FSNK0GP000038GDBZ91HT6SQ/2x.webp",
            "https://static-cdn.jtvnw.net/emoticons/v2/emotesv2_dcd06b30a5c24f6eb871e8f5edbd44f7/default/dark/2.0",
        ] {
            let frames = cache.get(url).await.unwrap().animation.map_or(1, |a| a.frames.len());
            println!("{frames} frames: {url}");
            assert!(frames > 1, "{url}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
