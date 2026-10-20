//! Network smoke test: `cargo run --example chat_spike <vod_id> <start_s> <end_s>`.

use wclip_lib::twitch::chat;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [vod, start, end] = args.as_slice() else {
        eprintln!("usage: chat_spike <vod_id> <start_s> <end_s>");
        std::process::exit(2);
    };
    let (start, end): (f64, f64) = (start.parse().unwrap(), end.parse().unwrap());

    let client = reqwest::Client::new();
    let started = std::time::Instant::now();
    let comments = chat::fetch_comments(&client, vod, start, end, |_| {}).await.unwrap();

    println!("{} comments in [{start}, {end}), {:.1} s", comments.len(), started.elapsed().as_secs_f64());
    let mut ids: Vec<&str> = comments.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    println!("{} unique", ids.len());
    for c in comments.iter().take(5) {
        let name = c.commenter.as_ref().map_or("<deleted>", |u| u.display_name.as_str());
        let text: String = c.message.fragments.iter().map(|f| f.text.as_str()).collect();
        println!("{:>7.1} {name}: {text}", c.content_offset_seconds);
    }
    // Largest silence between consecutive comments: a big gap in a busy chat means a lost page.
    let gap = comments
        .windows(2)
        .map(|w| w[1].content_offset_seconds - w[0].content_offset_seconds)
        .fold(0.0, f64::max);
    println!("max gap {gap:.0} s");
}
