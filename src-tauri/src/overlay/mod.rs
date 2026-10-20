//! Chat overlay: a transparent panel with the chat feed, rendered frame by frame and
//! piped into ffmpeg, which places it over the video.

pub mod feed;
pub mod text;

use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};

use crate::chat::ChatMessage;
use feed::Feed;
use text::Typesetter;

/// Overlay frame rate, that of most Twitch VODs: at 30 a scrolling chat stutters
/// over 60 fps video. ffmpeg matches it to the video by timestamps.
pub const FPS: u32 = 60;

/// Overlay preset. Sizes are pixels at 1080p and scale with the video height.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlayStyle {
    /// Top-left corner of the panel; ffmpeg keeps the panel inside the frame.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Font family for text; emoji and missing glyphs fall back to other fonts.
    pub font: String,
    pub font_size: f32,
    /// Panel background opacity, 0..=1.
    pub background: f32,
    /// Seconds a message stays; 0 keeps it until pushed out.
    pub lifetime: f64,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            // Bottom-left, 24 px from the edges.
            x: 24,
            y: 1080 - 520 - 24,
            width: 440,
            height: 520,
            font: text::FONT_FAMILY.to_owned(),
            font_size: 22.0,
            background: 0.0,
            lifetime: 0.0,
        }
    }
}

impl OverlayStyle {
    pub fn scaled(&self, video_height: u32) -> Self {
        let k = video_height as f32 / 1080.0;
        let px = |v: u32| (v as f32 * k).round() as u32;
        Self {
            x: px(self.x),
            y: px(self.y),
            width: px(self.width).max(2) & !1,
            height: px(self.height).max(2) & !1,
            font_size: self.font_size * k,
            ..self.clone()
        }
    }

    /// ffmpeg `overlay` filter position, quoted. A panel past the right or bottom
    /// edge (a wider preset on a 4:3 video) is moved back inside.
    pub fn position(&self) -> (String, String) {
        (format!("'min({},main_w-overlay_w)'", self.x), format!("'min({},main_h-overlay_h)'", self.y))
    }

    /// How much chat before the clip start to load so the panel starts filled.
    pub fn lookback(&self) -> f64 {
        if self.lifetime > 0.0 { self.lifetime + feed::FADE } else { 60.0 }
    }
}

pub struct Overlay {
    style: OverlayStyle,
    messages: Vec<ChatMessage>,
    images: HashMap<String, RgbaImage>,
    feed: Feed,
    typesetter: Typesetter,
    /// Rendered messages by index; dropped once scrolled away.
    blocks: HashMap<usize, Pixmap>,
    canvas: Pixmap,
    last: Option<(Vec<feed::Placed>, Arc<Vec<u8>>)>,
}

impl Overlay {
    /// `style` already scaled to the video. `messages` sorted by time.
    pub fn new(style: OverlayStyle, messages: Vec<ChatMessage>, images: HashMap<String, RgbaImage>) -> Self {
        let pad = Self::padding(&style);
        let times: Vec<f64> = messages.iter().map(|m| m.time).collect();
        let feed = Feed {
            times: Feed::spread(&times),
            gap: (style.font_size * 0.3).round(),
            bottom: style.height as f32 - pad,
            lifetime: (style.lifetime > 0.0).then_some(style.lifetime),
        };
        let canvas = Pixmap::new(style.width, style.height).expect("non-zero overlay size");
        Self {
            style,
            messages,
            images,
            feed,
            typesetter: Typesetter::new(),
            blocks: HashMap::new(),
            canvas,
            last: None,
        }
    }

    fn padding(style: &OverlayStyle) -> f32 {
        (style.font_size * 0.5).round()
    }

    /// Straight-alpha RGBA of the panel at VOD time `t`. A still feed returns the
    /// previous buffer without drawing.
    pub fn frame(&mut self, t: f64) -> Arc<Vec<u8>> {
        let pad = Self::padding(&self.style);
        let width = self.style.width - 2 * pad as u32;
        let (messages, images, style) = (&self.messages, &self.images, &self.style);
        let (blocks, typesetter) = (&mut self.blocks, &mut self.typesetter);
        let placed = self.feed.at(t, |i| {
            blocks
                .entry(i)
                .or_insert_with(|| typesetter.render(&messages[i], images, &style.font, style.font_size, width))
                .height() as f32
        });

        if let Some((prev, buf)) = &self.last
            && *prev == placed
        {
            return Arc::clone(buf);
        }

        let oldest = placed.last().map_or(usize::MAX, |p| p.index);
        self.blocks.retain(|&i, _| i >= oldest);

        let alpha = (self.style.background.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.canvas.fill(Color::from_rgba8(0, 0, 0, alpha));
        for p in placed.iter().rev() {
            let paint = PixmapPaint { opacity: f32::from(p.alpha) / 255.0, ..PixmapPaint::default() };
            let block = &self.blocks[&p.index];
            self.canvas.draw_pixmap(pad as i32, p.y, block.as_ref(), &paint, Transform::identity(), None);
        }

        let buf = Arc::new(demultiply(self.canvas.data()));
        self.last = Some((placed, Arc::clone(&buf)));
        buf
    }
}

/// The panel in `style` at 1080p, filled with made-up messages: settings preview
/// without loading any chat.
pub fn sample_panel(style: &OverlayStyle) -> RgbaImage {
    let style = style.scaled(1080);
    let (w, h) = (style.width, style.height);
    let messages = sample_messages();
    let t = messages.last().map_or(0.0, |m| m.time) + 1.0;
    let mut overlay = Overlay::new(style, messages, HashMap::new());
    RgbaImage::from_raw(w, h, overlay.frame(t).to_vec()).expect("frame size")
}

fn sample_messages() -> Vec<ChatMessage> {
    use crate::chat::Part;
    [
        ("Nightbot", [0x8A, 0x2B, 0xE2], "Welcome to the stream! 👋"),
        ("kotik_42", [0xFF, 0x69, 0xB4], "привет чат"),
        ("SpeedRunner", [0x1E, 0x90, 0xFF], "that jump was frame perfect, no way 😮"),
        ("mira", [0x9A, 0xCD, 0x32], "GG"),
        ("Ленивец", [0xDA, 0xA5, 0x20], "ставлю лайк, ждём хайлайты 🔥"),
        ("pixel_pusher", [0xFF, 0x45, 0x00], "who else is watching this at 3am"),
        ("Zed", [0x00, 0xFF, 0x7F], "LUL"),
        ("quiet_viewer", [0x5F, 0x9E, 0xA0], "first time here, the chat overlay looks clean"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, color, text))| ChatMessage {
        time: i as f64,
        name: name.into(),
        color,
        badges: Vec::new(),
        parts: vec![Part::Text(text.into())],
    })
    .collect()
}

fn demultiply(premultiplied: &[u8]) -> Vec<u8> {
    let mut out = premultiplied.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Part;

    fn msg(time: f64, name: &str, color: [u8; 3], parts: Vec<Part>) -> ChatMessage {
        ChatMessage { time, name: name.into(), color, badges: vec!["badge".into()], parts }
    }

    fn sample() -> (Vec<ChatMessage>, HashMap<String, RgbaImage>) {
        let emote = |name: &str| Part::Emote { name: name.into(), url: name.into() };
        let text = |s: &str| Part::Text(s.into());
        let messages = vec![
            msg(0.0, "Alice", [0xFF, 0x7F, 0x50], vec![text("hello chat "), emote("square"), text(" wide one "), emote("wide")]),
            msg(1.0, "bob", [0, 0, 0xFF], vec![text("a long message that has to wrap over more than one line in this narrow panel, really")]),
            msg(1.0, "Кириллица", [0x9A, 0xCD, 0x32], vec![text("привет 日本語 emoji 😀🎉 ok")]),
            msg(2.0, "carol", [0xDA, 0xA5, 0x20], vec![text("missing image → "), emote("nope")]),
        ];
        let images = HashMap::from([
            ("badge".to_owned(), RgbaImage::from_pixel(36, 36, image::Rgba([145, 70, 255, 255]))),
            ("square".to_owned(), RgbaImage::from_pixel(56, 56, image::Rgba([0, 200, 120, 255]))),
            ("wide".to_owned(), RgbaImage::from_pixel(168, 56, image::Rgba([230, 60, 60, 255]))),
        ]);
        (messages, images)
    }

    #[test]
    fn still_feed_reuses_buffer() {
        let (messages, images) = sample();
        let mut overlay = Overlay::new(OverlayStyle::default(), messages, images);
        let a = overlay.frame(10.0);
        let b = overlay.frame(10.5);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.len(), 440 * 520 * 4);
        let c = overlay.frame(2.1);
        assert!(!Arc::ptr_eq(&a, &c), "message 3 still sliding in");
    }

    #[test]
    fn scaled_style_is_even() {
        let s = OverlayStyle::default().scaled(720);
        assert_eq!((s.x, s.y, s.width, s.height), (16, 357, 292, 346));
        assert!((s.font_size - 22.0 * 720.0 / 1080.0).abs() < 1e-4);
    }

    /// Frames compared with `fixtures/snapshots/*.png`. Only embedded fonts are used
    /// (Latin, emoji), so the result does not depend on the machine's fonts.
    /// `WCLIP_UPDATE_SNAPSHOTS=1 cargo test snapshots` rewrites the references.
    #[test]
    fn snapshots() {
        let (mut messages, images) = sample();
        messages.retain(|m| m.name != "Кириллица");
        messages.push(msg(3.0, "dave", [0x1E, 0x90, 0xFF], vec![Part::Text("emoji 😀🎉 ok".into())]));
        let mut overlay = Overlay::new(OverlayStyle::default(), messages, images);
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/snapshots");
        let update = std::env::var_os("WCLIP_UPDATE_SNAPSHOTS").is_some();
        for t in [0.1, 3.1, 10.0] {
            let style = OverlayStyle::default();
            let frame = RgbaImage::from_raw(style.width, style.height, overlay.frame(t).to_vec()).unwrap();
            let path = format!("{dir}/overlay-{t}.png");
            if update {
                std::fs::create_dir_all(dir).unwrap();
                frame.save(&path).unwrap();
                continue;
            }
            let reference = image::open(&path).expect("reference missing, see the doc comment").to_rgba8();
            let differ = frame
                .pixels()
                .zip(reference.pixels())
                .filter(|(a, b)| a.0.iter().zip(b.0).any(|(x, y)| x.abs_diff(y) > 2))
                .count();
            assert!(differ <= 20, "t = {t}: {differ} pixels differ from {path}");
        }
    }

    /// Render speed on a busy chat (3 messages/s), release build:
    /// `cargo test --release -- --ignored --nocapture overlay_speed`.
    #[test]
    #[ignore]
    fn overlay_speed() {
        let (sample, images) = sample();
        let messages: Vec<ChatMessage> = (0..600)
            .map(|i| ChatMessage { time: f64::from(i / 3), ..sample[i as usize % sample.len()].clone() })
            .collect();
        let started = std::time::Instant::now();
        let mut overlay = Overlay::new(OverlayStyle::default(), messages, images);
        println!("setup {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let frames = 60 * FPS;
        for k in 0..frames {
            overlay.frame(100.0 + f64::from(k) / f64::from(FPS));
        }
        let per_frame = started.elapsed() / frames;
        println!("{per_frame:?} per frame, {:.0} fps", 1.0 / per_frame.as_secs_f64());
    }

    /// Writes the sample feed to `target/overlay-sample.png` for a visual check:
    /// `cargo test -- --ignored overlay_sample_png`.
    #[test]
    #[ignore]
    fn overlay_sample_png() {
        let (messages, images) = sample();
        let style = OverlayStyle { background: 0.35, ..OverlayStyle::default() };
        let (w, h) = (style.width, style.height);
        let mut overlay = Overlay::new(style, messages, images);
        let rgba = overlay.frame(10.0);
        let img = RgbaImage::from_raw(w, h, rgba.to_vec()).unwrap();
        img.save(concat!(env!("CARGO_MANIFEST_DIR"), "/target/overlay-sample.png")).unwrap();
    }
}
