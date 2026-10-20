//! Subtitles from spoken words: short captions, the word being said highlighted,
//! drawn by libass (ffmpeg `ass` filter) from an ASS file.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::overlay::text::INTER_BOLD;
use crate::speech::Word;

/// A caption on screen; times are seconds of the clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// When each word of `text` starts. After the text is edited the count may
    /// differ; the words then share the caption time evenly.
    pub words: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitleStyle {
    pub font: String,
    /// Pixels on a frame whose short side is 1080.
    pub size: f32,
    pub color: [u8; 3],
    /// The word being said; same as `color` for none.
    pub highlight: [u8; 3],
    /// Outline width in pixels at the same scale as `size`.
    pub outline: f32,
    pub uppercase: bool,
    /// Most words in a caption.
    pub words: usize,
    /// Height of the caption's middle in a 16:9 clip, as a fraction of the frame;
    /// vertical clips take it from the layout.
    pub y: f64,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font: crate::overlay::text::FONT_FAMILY.to_owned(),
            size: 64.0,
            color: [255, 255, 255],
            highlight: [255, 214, 0],
            outline: 5.0,
            uppercase: true,
            words: 3,
            y: 0.8,
        }
    }
}

/// A pause this long ends a caption.
const PAUSE: f64 = 0.6;
/// Shortest time a caption stays.
const MIN_SHOWN: f64 = 0.4;

/// Words grouped into captions of up to `max_words` (and about 8 letters a word),
/// ending early at a pause or the end of a sentence.
pub fn captions(words: &[Word], max_words: usize) -> Vec<Caption> {
    let max_words = max_words.max(1);
    let mut groups: Vec<Vec<&Word>> = Vec::new();
    for w in words {
        let open = groups.last().filter(|g| {
            let last = g[g.len() - 1];
            let chars: usize = g.iter().map(|w| w.text.chars().count() + 1).sum::<usize>() + w.text.chars().count();
            g.len() < max_words
                && chars <= 8 * max_words
                && w.start - last.end < PAUSE
                && !last.text.ends_with(['.', '!', '?', '…'])
        });
        match open {
            Some(_) => groups.last_mut().expect("open group").push(w),
            None => groups.push(vec![w]),
        }
    }
    let starts: Vec<f64> = groups.iter().map(|g| g[0].start).collect();
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let start = g[0].start;
            let end = g[g.len() - 1].end.max(start + MIN_SHOWN);
            // Up to the next caption when it is near, so the text does not blink.
            let end = match starts.get(i + 1) {
                Some(&next) if next - end < 0.3 => next,
                Some(&next) => end.min(next),
                None => end,
            };
            let text = g.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            Caption { start, end, text, words: g.iter().map(|w| w.start).collect() }
        })
        .collect()
}

/// ASS for a `width` x `height` clip with captions centred at `y` (fraction of the height).
pub fn ass(captions: &[Caption], style: &SubtitleStyle, width: u32, height: u32, y: f64) -> String {
    let k = width.min(height) as f32 / 1080.0;
    let bold = -1;
    let mut out = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, \
         Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,{},{},{},{},&H00000000,&H80000000,{bold},0,0,0,100,100,0,0,1,{},{},5,{},{},0,1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        style.font.replace(',', " "),
        (style.size * k).round(),
        ass_color(style.color),
        ass_color(style.color),
        (style.outline * k).round(),
        (style.outline * k / 2.0).round(),
        (60.0 * k).round(),
        (60.0 * k).round(),
    );
    let pos = format!("{{\\pos({},{})}}", width / 2, (y.clamp(0.0, 1.0) * f64::from(height)).round());
    for c in captions {
        let text = if style.uppercase { c.text.to_uppercase() } else { c.text.clone() };
        let words: Vec<String> = text.split_whitespace().map(ass_text).collect();
        if words.is_empty() {
            continue;
        }
        let times = word_times(c, words.len());
        for (i, &from) in times.iter().enumerate() {
            let to = times.get(i + 1).copied().unwrap_or(c.end);
            let line: Vec<String> = words
                .iter()
                .enumerate()
                .map(|(j, w)| {
                    if j == i && style.highlight != style.color {
                        format!("{{\\c{}}}{w}{{\\c{}}}", ass_color(style.highlight), ass_color(style.color))
                    } else {
                        w.clone()
                    }
                })
                .collect();
            let _ = writeln!(out, "Dialogue: 0,{},{},Default,,0,0,0,,{pos}{}", ass_time(from), ass_time(to), line.join(" "));
        }
    }
    out
}

/// When each of `n` words of `c` starts: as spoken, or shared evenly when the text was edited.
fn word_times(c: &Caption, n: usize) -> Vec<f64> {
    if c.words.len() == n && c.words.windows(2).all(|w| w[0] <= w[1]) {
        return c.words.iter().map(|t| t.clamp(c.start, c.end)).collect();
    }
    let step = (c.end - c.start) / n as f64;
    (0..n).map(|i| c.start + step * i as f64).collect()
}

/// Writes `captions` as `<dir>/<name>.ass` for `CutOptions::subtitles`, with the
/// bundled font in `<dir>/fonts`. `name` must be plain: letters, digits, `-`, `_`.
pub fn write(dir: &Path, name: &str, captions: &[Caption], style: &SubtitleStyle, size: (u32, u32), y: f64) -> std::io::Result<PathBuf> {
    let fonts = dir.join("fonts");
    std::fs::create_dir_all(&fonts)?;
    let font = fonts.join("Inter-Bold.ttf");
    if std::fs::metadata(&font).map(|m| m.len()).ok() != Some(INTER_BOLD.len() as u64) {
        std::fs::write(&font, INTER_BOLD)?;
    }
    let file = dir.join(format!("{name}.ass"));
    std::fs::write(&file, ass(captions, style, size.0, size.1, y))?;
    Ok(file)
}

/// `&HBBGGRR&`, opaque.
fn ass_color([r, g, b]: [u8; 3]) -> String {
    format!("&H00{b:02X}{g:02X}{r:02X}&")
}

/// `H:MM:SS.cc`.
fn ass_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
}

/// Braces would start an override block and a backslash an escape.
fn ass_text(word: &str) -> String {
    word.replace('\\', "/").replace('{', "(").replace('}', ")")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word { start, end, text: text.into() }
    }

    #[test]
    fn groups_words_into_captions() {
        let words = [
            word("Привет,", 0.0, 0.4),
            word("как", 0.4, 0.6),
            word("дела?", 0.6, 1.0),
            word("Отлично", 1.1, 1.5),
            word("всё", 3.0, 3.2),
            word("суперпупердлинноеслово", 3.2, 3.9),
        ];
        let c = captions(&words, 3);
        let texts: Vec<&str> = c.iter().map(|c| c.text.as_str()).collect();
        // A sentence ends; a pause; a word too long to share a caption.
        assert_eq!(texts, ["Привет, как дела?", "Отлично", "всё", "суперпупердлинноеслово"]);
        assert_eq!(c[0].words, [0.0, 0.4, 0.6]);
        // The next caption is near: the first stays until it.
        assert_eq!((c[0].start, c[0].end), (0.0, 1.1));
        assert_eq!((c[1].start, c[1].end), (1.1, 1.5));
        assert_eq!(c[2].end, 3.2);
        assert_eq!(captions(&words, 1).len(), words.len());
    }

    #[test]
    fn ass_highlights_each_word_in_turn() {
        let c = Caption { start: 1.0, end: 2.5, text: "hi {there}".into(), words: vec![1.0, 1.5] };
        let style = SubtitleStyle::default();
        let text = ass(std::slice::from_ref(&c), &style, 1080, 1920, 0.5);
        assert!(text.contains("PlayResX: 1080\nPlayResY: 1920\n"), "{text}");
        assert!(text.contains("Style: Default,Inter,64,&H00FFFFFF&,&H00FFFFFF&,&H00000000,&H80000000,-1,"), "{text}");
        let events: Vec<&str> = text.lines().filter(|l| l.starts_with("Dialogue:")).collect();
        assert_eq!(
            events,
            [
                "Dialogue: 0,0:00:01.00,0:00:01.50,Default,,0,0,0,,{\\pos(540,960)}{\\c&H0000D6FF&}HI{\\c&H00FFFFFF&} (THERE)",
                "Dialogue: 0,0:00:01.50,0:00:02.50,Default,,0,0,0,,{\\pos(540,960)}HI {\\c&H0000D6FF&}(THERE){\\c&H00FFFFFF&}",
            ]
        );
        // An edited text shares the time evenly.
        let edited = Caption { text: "one two three".into(), ..c };
        assert_eq!(word_times(&edited, 3), [1.0, 1.5, 2.0]);
        // A 16:9 frame scales by its height.
        assert!(ass(&[], &style, 1280, 720, 0.8).contains("Style: Default,Inter,43,"));
        assert_eq!(ass_time(3725.456), "1:02:05.46");
    }
}
