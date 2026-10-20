# Third-party components

## Bundled in the binary

- **Inter** by Rasmus Andersson, SIL Open Font License 1.1.
  Files: `src-tauri/assets/fonts/Inter-*.ttf` (chat overlay) and `static/fonts/Inter-*.ttf` (interface),
  license: `src-tauri/assets/fonts/Inter-LICENSE.txt`.
- **Twemoji Mozilla** font (emoji in chat). Font build: Mozilla, Apache License 2.0.
  Emoji graphics: Twemoji by Twitter, [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
  File: `src-tauri/assets/fonts/Twemoji.Mozilla.ttf`, license: `src-tauri/assets/fonts/Twemoji-LICENSE.md`.

- **whisper.cpp** by Georgi Gerganov and contributors (speech recognition for subtitles), MIT License,
  compiled in through the `whisper-rs` crate.

Rust and npm dependencies are listed in `src-tauri/Cargo.lock` and `package-lock.json` with their own licenses.

## Downloaded on first run, not bundled

- **FFmpeg** static builds from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), GPL.
- **yt-dlp** from [yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp), Unlicense.

- **Whisper large-v3-turbo** speech model by OpenAI, MIT License, in the ggml format of
  [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp); downloaded when subtitles are first used.

WClip runs FFmpeg and yt-dlp as separate programs.

## Online services

Chat, badges and emotes come from Twitch, 7TV, BetterTTV and FrankerFaceZ. WClip is not affiliated with any of them.
