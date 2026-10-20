//! Chat feed timeline: where each message sits and how opaque it is at a moment.
//! Newest message at the bottom; on arrival the stack slides up by its height while
//! the message fades in; with a lifetime set, old messages fade out at the top.

/// Slide and fade-in time of a new message, seconds.
pub const SLIDE: f64 = 0.2;
/// Fade-out time of an expired message, seconds.
pub const FADE: f64 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub index: usize,
    /// Top edge in panel pixels; negative when cut off by the panel top.
    pub y: i32,
    /// 0..=1, quantized so a still feed compares equal frame to frame.
    pub alpha: u8,
}

pub struct Feed {
    /// Arrival times, sorted.
    pub times: Vec<f64>,
    pub gap: f32,
    /// Bottom edge of the newest message.
    pub bottom: f32,
    /// `None`: messages stay until pushed out of the panel.
    pub lifetime: Option<f64>,
}

impl Feed {
    /// Comment offsets are whole seconds; spreading each second's messages evenly
    /// over that second keeps a busy chat from jumping in bursts.
    pub fn spread(times: &[f64]) -> Vec<f64> {
        let mut out = Vec::with_capacity(times.len());
        let mut i = 0;
        while i < times.len() {
            let n = times[i..].iter().take_while(|&&t| t == times[i]).count();
            out.extend((0..n).map(|k| times[i] + k as f64 / n as f64));
            i += n;
        }
        out
    }

    /// Visible messages at time `t`, newest first. `height(i)` is message `i`'s
    /// rendered height; it is only asked for messages that may be on screen.
    pub fn at(&self, t: f64, mut height: impl FnMut(usize) -> f32) -> Vec<Placed> {
        let arrived = self.times.partition_point(|&m| m <= t);
        let mut lift = 0.0;
        for i in (0..arrived).rev() {
            let age = t - self.times[i];
            if age >= SLIDE {
                break;
            }
            lift += (1.0 - ease(age / SLIDE)) as f32 * (height(i) + self.gap);
        }

        let mut out = Vec::new();
        let mut bottom = self.bottom + lift;
        for i in (0..arrived).rev() {
            if bottom <= 0.0 {
                break;
            }
            let age = t - self.times[i];
            let fade_out = match self.lifetime {
                Some(life) if age >= life + FADE => break,
                Some(life) => 1.0 - ((age - life) / FADE).clamp(0.0, 1.0),
                None => 1.0,
            };
            let alpha = ease((age / SLIDE).min(1.0)).min(fade_out);
            let h = height(i);
            let top = bottom - h;
            out.push(Placed { index: i, y: top.round() as i32, alpha: (alpha * 255.0).round() as u8 });
            bottom = top - self.gap;
        }
        out
    }
}

/// Ease-out cubic.
fn ease(x: f64) -> f64 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(times: &[f64], lifetime: Option<f64>) -> Feed {
        Feed { times: times.to_vec(), gap: 4.0, bottom: 100.0, lifetime }
    }

    #[test]
    fn spreads_within_second() {
        assert_eq!(Feed::spread(&[5.0, 5.0, 5.0, 5.0, 7.0]), vec![5.0, 5.25, 5.5, 5.75, 7.0]);
    }

    #[test]
    fn stack_from_bottom_newest_first() {
        let f = feed(&[1.0, 2.0, 3.0], None);
        let placed = f.at(10.0, |_| 20.0);
        assert_eq!(
            placed,
            vec![
                Placed { index: 2, y: 80, alpha: 255 },
                Placed { index: 1, y: 56, alpha: 255 },
                Placed { index: 0, y: 32, alpha: 255 },
            ]
        );
        assert!(f.at(0.5, |_| 20.0).is_empty(), "nothing before the first message");
    }

    #[test]
    fn new_message_slides_in() {
        let f = feed(&[1.0, 5.0], None);
        let start = f.at(5.0, |_| 20.0);
        assert_eq!(start[0], Placed { index: 1, y: 104, alpha: 0 }, "below the panel bottom, invisible");
        assert_eq!(start[1].y, 80, "older one has not moved yet");
        let end = f.at(5.0 + SLIDE, |_| 20.0);
        assert_eq!(end[0], Placed { index: 1, y: 80, alpha: 255 });
        assert_eq!(end[1].y, 56);
        let mid = f.at(5.0 + SLIDE / 2.0, |_| 20.0);
        assert!(mid[0].y < 104 && mid[0].y > 80 && mid[0].alpha > 0 && mid[0].alpha < 255);
    }

    #[test]
    fn stops_at_panel_top() {
        let f = feed(&(0..50).map(f64::from).collect::<Vec<_>>(), None);
        let mut asked = Vec::new();
        let placed = f.at(100.0, |i| {
            asked.push(i);
            40.0
        });
        assert_eq!(placed.len(), 3, "100 px holds 2 messages of 40 + 4, the third cut off");
        assert!(placed[2].y < 0);
        assert_eq!(asked.len(), 3, "heights of hidden messages are never needed");
    }

    #[test]
    fn lifetime_fades_out() {
        let f = feed(&[0.0, 10.0], Some(5.0));
        assert_eq!(f.at(5.0, |_| 20.0)[0].alpha, 255);
        let fading = f.at(5.0 + FADE / 2.0, |_| 20.0);
        assert!(fading[0].alpha > 0 && fading[0].alpha < 255);
        assert!(f.at(5.0 + FADE, |_| 20.0).is_empty());
        assert_eq!(f.at(11.0, |_| 20.0).len(), 1, "only the new one");
    }
}
