//! Finding moments worth clipping from the chat of a whole VOD: how busy the chat
//! is over time, and where it bursts.
//!
//! A peak is a stretch where the chat runs well above its usual pace around it
//! (a rolling median), so a quiet stream's spikes rank as high as a busy one's.

use std::collections::HashMap;

use serde::Serialize;

use crate::chat::{ChatMessage, Part};

/// Chart bars aim for about this many; the bar width is rounded to a nice step.
const BARS: f64 = 480.0;
const BAR_STEPS: [f64; 10] = [5.0, 10.0, 15.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0];
/// Resolution of peak detection, seconds.
const STEP: f64 = 5.0;
/// Moving average over this many steps smooths single busy seconds out.
const SMOOTH: usize = 5;
/// The usual pace is the median over this many steps on each side (5 min).
const BASELINE: usize = 60;
/// Peaks closer than this are one moment.
const MIN_GAP: f64 = 90.0;
const MAX_PEAKS: usize = 20;
/// Messages within the smoothing window a peak needs.
const MIN_MESSAGES: f64 = 5.0;
/// Words listed per peak.
const TOP_WORDS: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// Seconds per bar of `counts`.
    pub bar: f64,
    /// Matching messages per bar, from 0:00.
    pub counts: Vec<u32>,
    /// Matching messages in the whole video.
    pub total: u32,
    /// Strongest first.
    pub peaks: Vec<Peak>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peak {
    /// The burst of chat, seconds of the video; the moment itself is a little earlier.
    pub start: f64,
    pub end: f64,
    /// Matching messages in the burst.
    pub count: u32,
    /// Most used words and emotes of the burst, with the number of messages using each.
    pub words: Vec<(String, u32)>,
}

/// Text of a message as typed: emotes by name.
pub fn message_text(m: &ChatMessage) -> String {
    m.parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.as_str(),
            Part::Emote { name, .. } => name.as_str(),
        })
        .collect()
}

/// Search terms from what the user typed: comma-separated, case-insensitive.
pub fn parse_query(query: &str) -> Vec<String> {
    query.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect()
}

/// A message matches when its text contains any of `terms`; no terms match all.
fn matches(text_lower: &str, terms: &[String]) -> bool {
    terms.is_empty() || terms.iter().any(|t| text_lower.contains(t.as_str()))
}

fn bar_width(duration: f64) -> f64 {
    let ideal = duration / BARS;
    BAR_STEPS.into_iter().find(|&s| s >= ideal).unwrap_or(BAR_STEPS[BAR_STEPS.len() - 1])
}

/// Chat activity of messages matching `query` (see `parse_query`) over a video of
/// `duration` seconds. Messages before 0:00 or after the end are left out.
pub fn activity(messages: &[ChatMessage], duration: f64, query: &str) -> Activity {
    let terms = parse_query(query);
    let hits: Vec<&ChatMessage> = messages
        .iter()
        .filter(|m| m.time >= 0.0 && m.time < duration)
        .filter(|m| matches(&message_text(m).to_lowercase(), &terms))
        .collect();

    let bar = bar_width(duration);
    let mut counts = vec![0u32; (duration / bar).ceil().max(1.0) as usize];
    let mut steps = vec![0u32; (duration / STEP).ceil().max(1.0) as usize];
    let (last_bar, last_step) = (counts.len() - 1, steps.len() - 1);
    for m in &hits {
        counts[((m.time / bar) as usize).min(last_bar)] += 1;
        steps[((m.time / STEP) as usize).min(last_step)] += 1;
    }
    Activity { bar, counts, total: hits.len() as u32, peaks: peaks(&steps, &hits) }
}

fn peaks(steps: &[u32], hits: &[&ChatMessage]) -> Vec<Peak> {
    let n = steps.len();
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(SMOOTH / 2), (i + SMOOTH / 2 + 1).min(n));
            steps[a..b].iter().map(|&c| f64::from(c)).sum::<f64>() / (b - a) as f64
        })
        .collect();
    let baseline: Vec<f64> = (0..n)
        .map(|i| {
            let mut window = smooth[i.saturating_sub(BASELINE)..(i + BASELINE + 1).min(n)].to_vec();
            window.sort_by(f64::total_cmp);
            window[window.len() / 2]
        })
        .collect();
    let excess: Vec<f64> = (0..n).map(|i| smooth[i] - baseline[i]).collect();

    let mut order: Vec<usize> = (0..n).filter(|&i| excess[i] > 0.0).collect();
    order.sort_by(|&a, &b| excess[b].total_cmp(&excess[a]).then(a.cmp(&b)));
    let mut picked: Vec<usize> = Vec::new();
    for i in order {
        if picked.len() == MAX_PEAKS {
            break;
        }
        // A moment at least doubles the usual pace; a few messages in a dead chat
        // are noise.
        if smooth[i] < 2.0 * baseline[i] || smooth[i] * (SMOOTH as f64) < MIN_MESSAGES {
            continue;
        }
        if picked.iter().all(|&p| (p as f64 - i as f64).abs() * STEP >= MIN_GAP) {
            picked.push(i);
        }
    }

    picked
        .into_iter()
        .map(|i| {
            // The burst: neighbours still at least halfway up from the baseline.
            let floor = baseline[i] + excess[i] / 2.0;
            let mut a = i;
            while a > 0 && smooth[a - 1] >= floor {
                a -= 1;
            }
            let mut b = i;
            while b + 1 < n && smooth[b + 1] >= floor {
                b += 1;
            }
            let (start, end) = (a as f64 * STEP, (b + 1) as f64 * STEP);
            let burst: Vec<&ChatMessage> = hits.iter().copied().filter(|m| m.time >= start && m.time < end).collect();
            Peak { start, end, count: burst.len() as u32, words: top_words(&burst) }
        })
        .collect()
}

/// Too common to tell one moment from another.
const STOP_WORDS: &[&str] = &[
    "the", "is", "it", "to", "and", "of", "in", "that", "this", "he", "she", "they", "so", "you", "my", "me", "what",
    "on", "for", "was", "be", "are", "его", "это", "что", "как", "на", "не", "по", "но", "ну", "он", "она", "они", "там",
];

/// Words used by the most messages, each counted once per message and regardless
/// of case (shown as first typed); at least two. Mentions and stop words are left out.
fn top_words(messages: &[&ChatMessage]) -> Vec<(String, u32)> {
    let mut counts: HashMap<String, (String, u32)> = HashMap::new();
    for m in messages {
        let text = message_text(m);
        let mut words: Vec<(String, &str)> = text
            .split_whitespace()
            .filter(|w| w.chars().count() > 1 && !w.starts_with('@'))
            .map(|w| (w.to_lowercase(), w))
            .filter(|(lower, _)| !STOP_WORDS.contains(&lower.as_str()))
            .collect();
        words.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        words.dedup_by(|a, b| a.0 == b.0);
        for (lower, word) in words {
            counts.entry(lower).or_insert_with(|| (word.to_owned(), 0)).1 += 1;
        }
    }
    let mut top: Vec<(String, u32)> = counts.into_values().filter(|&(_, c)| c >= 2).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(TOP_WORDS);
    top
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(time: f64, text: &str) -> ChatMessage {
        ChatMessage { time, name: "u".into(), color: [255; 3], badges: Vec::new(), parts: vec![Part::Text(text.into())] }
    }

    /// A message every 10 s for an hour, plus bursts of `size` messages over 10 s.
    fn chat(bursts: &[(f64, usize, &str)]) -> Vec<ChatMessage> {
        let mut all: Vec<ChatMessage> = (0..360).map(|i| msg(f64::from(i) * 10.0, "hello there")).collect();
        for &(at, size, text) in bursts {
            all.extend((0..size).map(|k| msg(at + k as f64 * 10.0 / size as f64, text)));
        }
        all.sort_by(|a, b| a.time.total_cmp(&b.time));
        all
    }

    #[test]
    fn bars() {
        assert_eq!(bar_width(600.0), 5.0);
        assert_eq!(bar_width(3600.0), 10.0);
        assert_eq!(bar_width(8.0 * 3600.0), 60.0);
        let a = activity(&chat(&[]), 3600.0, "");
        assert_eq!((a.bar, a.counts.len(), a.total), (10.0, 360, 360));
        assert!(a.counts.iter().all(|&c| c == 1));
    }

    #[test]
    fn finds_bursts_strongest_first() {
        let all = chat(&[(1200.0, 40, "KEKW KEKW"), (2400.0, 80, "OMEGALUL no way")]);
        let a = activity(&all, 3600.0, "");
        assert_eq!(a.peaks.len(), 2, "{:?}", a.peaks);
        let (first, second) = (&a.peaks[0], &a.peaks[1]);
        assert!(first.start <= 2400.0 && first.end >= 2410.0, "{first:?}");
        assert!(first.end - first.start <= 40.0, "burst is tight: {first:?}");
        assert!(second.start <= 1200.0 && second.end >= 1210.0, "{second:?}");
        assert_eq!(first.words[0], ("OMEGALUL".to_owned(), 80));
        assert_eq!(second.words[0], ("KEKW".to_owned(), 40), "counted once per message");
    }

    #[test]
    fn steady_chat_has_no_peaks() {
        assert!(activity(&chat(&[]), 3600.0, "").peaks.is_empty());
    }

    #[test]
    fn query_filters() {
        let all = chat(&[(1200.0, 40, "KEKW"), (2400.0, 80, "OMEGALUL")]);
        let a = activity(&all, 3600.0, "kekw, nothing");
        assert_eq!(a.total, 40);
        assert_eq!(a.peaks.len(), 1);
        assert!(a.peaks[0].start <= 1200.0 && a.peaks[0].end >= 1210.0);
        assert_eq!(parse_query(" A, ,b "), ["a", "b"]);
    }

    /// Peaks of a real hour of chat: `cargo test --release -- --ignored smoke_moments --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_moments() {
        let started = std::time::Instant::now();
        let all = crate::chat::load(&reqwest::Client::new(), "2905205256", 0.0, 3600.0, |_| {}).await.unwrap();
        println!("{} messages in {:.1} s", all.len(), started.elapsed().as_secs_f64());
        let started = std::time::Instant::now();
        let a = activity(&all, 3600.0, "");
        println!("activity in {:?}, bar {} s", started.elapsed(), a.bar);
        for p in &a.peaks {
            println!("{:>5.0}-{:<5.0} {:>4} {:?}", p.start, p.end, p.count, p.words);
        }
        assert!(!a.peaks.is_empty());
    }

    #[test]
    fn outside_video_dropped() {
        let all = vec![msg(-5.0, "early"), msg(1.0, "in"), msg(100.0, "late")];
        let a = activity(&all, 60.0, "");
        assert_eq!(a.total, 1);
    }
}
