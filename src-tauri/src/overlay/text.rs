//! Renders one chat message into a transparent pixmap.
//!
//! Line wrapping is cosmic-text's. Badges and emotes take part in it as placeholder
//! glyphs: an `M` tagged through `Attrs::metadata`, widened by letter spacing to the
//! image width and given a taller line. After layout the placeholders are skipped
//! and the images drawn where they landed.

use std::collections::HashMap;
use std::sync::LazyLock;

use cosmic_text::{
    Attrs, Buffer, Color, Fallback, Family, FontSystem, Metrics, PlatformFallback, Shaping, SwashCache,
    Weight, Wrap,
};
use image::RgbaImage;
use image::imageops::{self, FilterType};
use tiny_skia::{IntSize, Pixmap, PixmapPaint, PremultipliedColorU8, Transform};

use crate::chat::{ChatMessage, Part};

/// Embedded default font; placeholders always use it, whatever the text font.
pub const FONT_FAMILY: &str = "Inter";
const EMOJI_FAMILY: &str = "Twemoji Mozilla";
const PLACEHOLDER: &str = "M";

/// Sizes relative to the font size, close to Twitch's web chat.
const LINE_HEIGHT: f32 = 1.45;
const EMOTE_HEIGHT: f32 = 2.0;
const BADGE_HEIGHT: f32 = 1.3;
const BADGE_GAP: f32 = 0.25;
/// Outline radius is the font size divided by this, at least 2 px.
const OUTLINE: f32 = 9.0;

pub struct Typesetter {
    fonts: FontSystem,
    cache: SwashCache,
    /// Advance of the placeholder glyph, in em.
    placeholder_em: f32,
    /// Scaled images by (url, height).
    scaled: HashMap<(String, u32), Pixmap>,
}

/// Emoji come from our Twemoji font on every OS, then the platform's usual list.
struct EmojiFirst;

static COMMON_FALLBACK: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    std::iter::once(EMOJI_FAMILY).chain(PlatformFallback.common_fallback().iter().copied()).collect()
});

impl Fallback for EmojiFirst {
    fn common_fallback(&self) -> &[&'static str] {
        &COMMON_FALLBACK
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        PlatformFallback.forbidden_fallback()
    }

    fn script_fallback(&self, script: unicode_script::Script, locale: &str) -> &[&'static str] {
        PlatformFallback.script_fallback(script, locale)
    }
}

/// One placeholder: an image and its size on screen.
struct Slot {
    url: String,
    width: u32,
    height: u32,
}

/// Scanning the system fonts takes a while, so it is done once and cloned.
static FONT_DB: LazyLock<fontdb::Database> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    // System fonts are the fallback for scripts Inter lacks (CJK and others).
    db.load_system_fonts();
    db.load_font_data(include_bytes!("../../assets/fonts/Inter-Regular.ttf").to_vec());
    db.load_font_data(include_bytes!("../../assets/fonts/Inter-Bold.ttf").to_vec());
    db.load_font_data(include_bytes!("../../assets/fonts/Twemoji.Mozilla.ttf").to_vec());
    db
});

impl Typesetter {
    /// Scans the fonts ahead of the first overlay; call from a background thread.
    pub fn warm_up() {
        LazyLock::force(&FONT_DB);
    }

    pub fn new() -> Self {
        let mut fonts = FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), FONT_DB.clone(), EmojiFirst);

        let mut probe = Buffer::new(&mut fonts, Metrics::new(100.0, 120.0));
        probe.set_text(PLACEHOLDER, &Attrs::new().family(Family::Name(FONT_FAMILY)), Shaping::Advanced, None);
        probe.shape_until_scroll(&mut fonts, false);
        let placeholder_em = probe.layout_runs().flat_map(|r| r.glyphs).map(|g| g.w).sum::<f32>() / 100.0;

        Self { fonts, cache: SwashCache::new(), placeholder_em, scaled: HashMap::new() }
    }

    /// Font families available for text: embedded ones and the system's.
    pub fn families(&self) -> Vec<String> {
        let mut out: Vec<String> =
            self.fonts.db().faces().filter_map(|f| f.families.first().map(|(name, _)| name.clone())).collect();
        out.sort_unstable_by_key(|f| f.to_lowercase());
        out.dedup();
        out
    }

    /// Message laid out to `width` pixels in `family` (missing glyphs fall back to
    /// other fonts); images missing from `images` (failed downloads) fall back to
    /// the emote name as text, or no badge.
    pub fn render(
        &mut self,
        msg: &ChatMessage,
        images: &HashMap<String, RgbaImage>,
        family: &str,
        font_size: f32,
        width: u32,
    ) -> Pixmap {
        let text_metrics = Metrics::new(font_size, (font_size * LINE_HEIGHT).round());
        let base = Attrs::new().family(Family::Name(family)).color(Color::rgba(255, 255, 255, 255));
        let mut slots: Vec<Slot> = Vec::new();
        let mut spans: Vec<(String, Attrs)> = Vec::new();

        let placeholder = |url: &str, height: f32, gap: f32, slots: &mut Vec<Slot>| -> Option<Attrs<'static>> {
            let img = images.get(url)?;
            let height = height.round().max(1.0) as u32;
            let width = ((img.width() as f32 * height as f32 / img.height().max(1) as f32).round() as u32).max(1);
            slots.push(Slot { url: url.to_owned(), width, height });
            let spacing = (width as f32 + gap) / font_size - self.placeholder_em;
            Some(
                Attrs::new()
                    .family(Family::Name(FONT_FAMILY))
                    .metadata(slots.len())
                    .letter_spacing(spacing)
                    .metrics(Metrics::new(font_size, (height as f32 + 2.0).max(text_metrics.line_height))),
            )
        };

        for url in &msg.badges {
            if let Some(attrs) = placeholder(url, font_size * BADGE_HEIGHT, font_size * BADGE_GAP, &mut slots) {
                spans.push((PLACEHOLDER.into(), attrs));
            }
        }
        let [r, g, b] = readable(msg.color);
        spans.push((msg.name.clone(), base.clone().weight(Weight::BOLD).color(Color::rgba(r, g, b, 255))));
        spans.push((": ".into(), base.clone()));
        for part in &msg.parts {
            match part {
                Part::Text(text) => spans.push((text.clone(), base.clone())),
                Part::Emote { name, url } => match placeholder(url, font_size * EMOTE_HEIGHT, 0.0, &mut slots) {
                    Some(attrs) => spans.push((PLACEHOLDER.into(), attrs)),
                    None => spans.push((name.clone(), base.clone())),
                },
            }
        }

        let mut buffer = Buffer::new(&mut self.fonts, text_metrics);
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(width as f32), None);
        buffer.set_rich_text(spans.iter().map(|(s, a)| (s.as_str(), a.clone())), &base, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);

        let height: f32 = buffer.layout_runs().map(|r| r.line_height).sum();
        let mut pixmap = Pixmap::new(width, (height.ceil() as u32).max(1)).expect("non-zero size");

        let mut glyphs = Vec::new();
        let mut emotes = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                if glyph.metadata > 0 {
                    let slot = &slots[glyph.metadata - 1];
                    let y = run.line_top + (run.line_height - slot.height as f32) / 2.0;
                    emotes.push((glyph.metadata - 1, glyph.x.round() as i32, y.round() as i32));
                } else {
                    let color = glyph.color_opt.unwrap_or(Color::rgba(255, 255, 255, 255));
                    glyphs.push((glyph.physical((0.0, run.line_y), 1.0), color));
                }
            }
        }

        // A solid black outline under all the text first, so the text reads on any
        // video and no outline covers a neighbouring letter.
        let r = (font_size / OUTLINE).round().max(2.0) as i32;
        for (physical, color) in &glyphs {
            for dy in -r..=r {
                for dx in -r..=r {
                    if (dx, dy) == (0, 0) || dx * dx + dy * dy > r * r + r {
                        continue;
                    }
                    self.cache.with_pixels(&mut self.fonts, physical.cache_key, *color, |x, y, c| {
                        blend(&mut pixmap, physical.x + x + dx, physical.y + y + dy, [0, 0, 0, c.a()]);
                    });
                }
            }
        }
        for (slot, x, y) in emotes {
            let image = self.scaled_image(&images[&slots[slot].url], &slots[slot]);
            pixmap.draw_pixmap(x, y, image.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        for (physical, color) in &glyphs {
            self.cache.with_pixels(&mut self.fonts, physical.cache_key, *color, |x, y, c| {
                blend(&mut pixmap, physical.x + x, physical.y + y, [c.r(), c.g(), c.b(), c.a()]);
            });
        }
        pixmap
    }

    fn scaled_image(&mut self, img: &RgbaImage, slot: &Slot) -> &Pixmap {
        self.scaled.entry((slot.url.clone(), slot.height)).or_insert_with(|| {
            let resized = imageops::resize(img, slot.width, slot.height, FilterType::CatmullRom);
            let mut data = resized.into_raw();
            for px in data.chunks_exact_mut(4) {
                let a = u16::from(px[3]);
                for c in &mut px[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
            Pixmap::from_vec(data, IntSize::from_wh(slot.width, slot.height).expect("non-zero"))
                .expect("size matches")
        })
    }
}

/// Source-over of a straight-alpha color onto the premultiplied pixmap.
fn blend(pixmap: &mut Pixmap, x: i32, y: i32, [r, g, b, a]: [u8; 4]) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    if a == 0 || x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let dst = &mut pixmap.pixels_mut()[(y * w + x) as usize];
    let a16 = u16::from(a);
    let inv = 255 - a16;
    let mix = |s: u8, d: u8| ((u16::from(s) * a16 + u16::from(d) * inv + 127) / 255) as u8;
    let out_a = ((a16 * 255 + u16::from(dst.alpha()) * inv + 127) / 255) as u8;
    *dst = PremultipliedColorU8::from_rgba(
        mix(r, dst.red()).min(out_a),
        mix(g, dst.green()).min(out_a),
        mix(b, dst.blue()).min(out_a),
        out_a,
    )
    .expect("clamped to alpha");
}

/// Lightens dark name colors so they stay readable over a dark outline, as Twitch
/// does in its dark theme.
fn readable([r, g, b]: [u8; 3]) -> [u8; 3] {
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if luma >= 100.0 {
        return [r, g, b];
    }
    let k = (100.0 - luma) / 255.0 * 1.6;
    let up = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * k.min(0.7)).round() as u8;
    [up(r), up(g), up(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_names_get_lighter() {
        assert_eq!(readable([0xFF, 0x7F, 0x50]), [0xFF, 0x7F, 0x50]);
        let blue = readable([0, 0, 0xFF]);
        assert!(blue[0] > 0 && blue[2] == 0xFF);
    }

    #[test]
    fn blend_over_transparent_and_opaque() {
        let mut p = Pixmap::new(2, 1).unwrap();
        blend(&mut p, 0, 0, [255, 0, 0, 128]);
        let px = p.pixels()[0];
        assert_eq!((px.red(), px.alpha()), (128, 128));
        blend(&mut p, 0, 0, [0, 0, 255, 255]);
        let px = p.pixels()[0];
        assert_eq!((px.red(), px.blue(), px.alpha()), (0, 255, 255));
        blend(&mut p, 5, 0, [0, 0, 255, 255]);
    }
}
