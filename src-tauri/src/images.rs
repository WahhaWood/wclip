//! Disk cache for badge and emote images. Images decode to RGBA; animated ones
//! (GIF, WebP) give their first frame.

use std::path::PathBuf;

use image::RgbaImage;

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

pub struct ImageCache {
    dir: PathBuf,
    client: reqwest::Client,
}

impl ImageCache {
    pub fn new(dir: PathBuf, client: reqwest::Client) -> Self {
        Self { dir, client }
    }

    pub async fn get(&self, url: &str) -> Result<RgbaImage> {
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
        Ok(image::load_from_memory(&bytes)?.to_rgba8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, ImageFormat, Rgba};

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
            let img = cache.get("https://x/e").await.unwrap();
            assert_eq!(img.dimensions(), (4, 3), "{name}");
            assert_eq!(*img.get_pixel(0, 0), RED, "{name}");
        }
    }

    #[tokio::test]
    async fn animated_gif_gives_first_frame() {
        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut bytes);
            let delay = Delay::from_numer_denom_ms(100, 1);
            for color in [RED, BLUE] {
                let frame = Frame::from_parts(RgbaImage::from_pixel(2, 2, color), 0, 0, delay);
                enc.encode_frame(frame).unwrap();
            }
        }
        let cache = cached("gif", "https://x/g", &bytes);
        let img = cache.get("https://x/g").await.unwrap();
        assert_eq!(*img.get_pixel(1, 1), RED);
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
            let img = cache.get(url).await.unwrap();
            assert!(img.width() > 0, "{url}");
            // Second call is served from disk.
            assert_eq!(cache.get(url).await.unwrap(), img);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
