//! Vertical clips: pieces of the source frame placed on a 9:16 canvas over a
//! background, with the chat panel somewhere on it.
//!
//! Pieces are cut by fractions of the source frame, so a layout fits any
//! resolution, and scaled by width only: ffmpeg keeps their proportions.

use serde::{Deserialize, Serialize};

pub const WIDTH: u32 = 1080;
pub const HEIGHT: u32 = 1920;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Layout {
    pub name: String,
    /// Bottom first.
    pub pieces: Vec<Piece>,
    /// The source blurred over the whole canvas; else `color`.
    pub blur: bool,
    pub color: [u8; 3],
    /// Where the chat panel goes, if the clip has chat.
    pub chat: ChatBox,
    /// Middle of the subtitles, as fractions of the canvas width and height.
    pub subtitles_x: f64,
    pub subtitles_y: f64,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            name: "Середина кадра".to_owned(),
            pieces: vec![Piece::default()],
            blur: true,
            color: [0, 0, 0],
            chat: ChatBox::default(),
            subtitles_x: 0.5,
            subtitles_y: 0.55,
        }
    }
}

/// A part of the source frame on the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Piece {
    pub name: String,
    /// Part of the source, as fractions of its width and height.
    pub src: Area,
    /// Top-left corner on the canvas; may stick out of it.
    pub x: i32,
    pub y: i32,
    /// Width on the canvas; the height follows the proportions of `src`.
    pub width: u32,
}

impl Default for Piece {
    /// The middle of a 16:9 frame at 9:16, filling the canvas.
    fn default() -> Self {
        let w = 9.0 / 16.0 * 9.0 / 16.0;
        Self { name: "Кадр".to_owned(), src: Area { x: (1.0 - w) / 2.0, y: 0.0, w, h: 1.0 }, x: 0, y: 0, width: WIDTH }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    /// Inside the frame and at least 1% of it on each side.
    fn clamped(self) -> Self {
        const MIN: f64 = 0.01;
        let w = self.w.clamp(MIN, 1.0);
        let h = self.h.clamp(MIN, 1.0);
        Self { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h }
    }
}

/// The chat panel on the canvas, in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub font_size: f32,
}

impl Default for ChatBox {
    fn default() -> Self {
        Self { x: 40, y: 1220, width: 1000, height: 560, font_size: 30.0 }
    }
}

impl Layout {
    /// Filters from the source `[0:v]` to the canvas labelled `out`.
    pub fn graph(&self, out: &str) -> String {
        let n = self.pieces.len();
        let mut parts = Vec::new();
        let splits: String = (0..n).map(|i| format!("[s{i}]")).collect();
        parts.push(format!("[0:v]split={}[sb]{splits}", n + 1));
        // The background is worked out small: blurring 1080x1920 every frame is slow.
        // Under the blur the crudest scaling looks the same and saves ~12% of the
        // filter time (measured 2026-10-21).
        let fill = if self.blur {
            "scale=270:480:force_original_aspect_ratio=increase:flags=fast_bilinear,crop=270:480,boxblur=10:2".to_owned()
        } else {
            let [r, g, b] = self.color;
            format!("scale=270:480:flags=fast_bilinear,drawbox=x=0:y=0:w=iw:h=ih:color=0x{r:02x}{g:02x}{b:02x}:t=fill")
        };
        parts.push(format!("[sb]{fill},scale={WIDTH}:{HEIGHT}:flags=fast_bilinear,setsar=1[l0]"));
        for (i, piece) in self.pieces.iter().enumerate() {
            let a = piece.src.clamped();
            let width = piece.width.clamp(2, 4 * WIDTH) & !1;
            parts.push(format!(
                "[s{i}]crop=w=iw*{}:h=ih*{}:x=iw*{}:y=ih*{},scale={width}:-2,setsar=1[p{i}]",
                a.w, a.h, a.x, a.y
            ));
            let next = if i + 1 == n { out.to_owned() } else { format!("l{}", i + 1) };
            parts.push(format!("[l{i}][p{i}]overlay=x={}:y={}[{next}]", piece.x, piece.y));
        }
        if n == 0 {
            parts.push(format!("[l0]null[{out}]"));
        }
        parts.join(";")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_places_pieces_in_order() {
        let cam = Piece { name: "cam".into(), src: Area { x: 0.75, y: 0.0, w: 0.25, h: 0.25 }, x: 0, y: 0, width: WIDTH };
        let game = Piece { name: "game".into(), src: Area { x: 0.2, y: 0.0, w: 0.6, h: 1.0 }, x: -10, y: 700, width: 1101 };
        let layout = Layout { pieces: vec![game, cam], ..Layout::default() };
        let parts: Vec<String> = layout.graph("v").split(';').map(str::to_owned).collect();
        assert_eq!(parts[0], "[0:v]split=3[sb][s0][s1]");
        assert!(parts[1].starts_with("[sb]scale=270:480:force_original_aspect_ratio=increase"), "{}", parts[1]);
        assert!(parts[1].ends_with("scale=1080:1920:flags=fast_bilinear,setsar=1[l0]"));
        assert_eq!(parts[2], "[s0]crop=w=iw*0.6:h=ih*1:x=iw*0.2:y=ih*0,scale=1100:-2,setsar=1[p0]");
        assert_eq!(parts[3], "[l0][p0]overlay=x=-10:y=700[l1]");
        assert_eq!(parts[5], "[l1][p1]overlay=x=0:y=0[v]");
    }

    #[test]
    fn reads_the_frontend_shape() {
        let json = r#"{"name": "cam", "blur": false, "color": [1, 2, 3],
            "pieces": [{"name": "Камера", "src": {"x": 0, "y": 0.6, "w": 0.19, "h": 0.4}, "x": 0, "y": -20, "width": 1080}],
            "chat": {"x": 40, "y": 1450, "width": 1000, "height": 440, "fontSize": 28}}"#;
        let layout: Layout = serde_json::from_str(json).unwrap();
        assert_eq!(layout.pieces[0].src, Area { x: 0.0, y: 0.6, w: 0.19, h: 0.4 });
        assert_eq!(layout.pieces[0].y, -20);
        assert_eq!(layout.chat.font_size, 28.0);
        assert_eq!(layout.color, [1, 2, 3]);
    }

    #[test]
    fn color_background_and_clamped_areas() {
        let wild = Piece { src: Area { x: 0.9, y: -1.0, w: 0.5, h: 2.0 }, ..Piece::default() };
        let layout = Layout { pieces: vec![wild], blur: false, color: [255, 16, 0], ..Layout::default() };
        let graph = layout.graph("v");
        assert!(graph.contains("color=0xff1000:t=fill"), "{graph}");
        assert!(graph.contains("crop=w=iw*0.5:h=ih*1:x=iw*0.5:y=ih*0,"), "{graph}");
        assert!(Layout { pieces: vec![], ..Layout::default() }.graph("v").ends_with("[l0]null[v]"));
    }
}
