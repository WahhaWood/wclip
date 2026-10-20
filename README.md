<div align="center">

<img src="assets/logo.svg" width="128" alt="WClip">

# WClip

**Нарезка записей Twitch с чатом в кадре: ссылка, тайминги, готовые MP4.**

![status](https://img.shields.io/badge/status-beta-orange)
![platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux-blue)
![license](https://img.shields.io/github/license/WahhaWood/wclip)

</div>

> [!WARNING]
> **Проект в бета-тесте.** Функциональность собирается по частям, многое ещё не работает или будет меняться.
> Готовых сборок пока нет, первые появятся в [Releases](https://github.com/WahhaWood/wclip/releases).
> Нашли ошибку — откройте [issue](https://github.com/WahhaWood/wclip/issues).

## Что это

Укажите ссылку на запись стрима и интервалы, а WClip вырежет их и наложит чат стрима поверх видео.

- Один файл без установки: `.exe` для Windows, AppImage для Linux.
- Только публичные записи Twitch, вход в аккаунт не нужен.
- Чат со смайлами Twitch, 7TV, BTTV и FFZ, бейджами и цветами ников.
- Настраиваемый оверлей: угол, размер, шрифт, прозрачность, время жизни сообщений; пресеты.
- ffmpeg и yt-dlp докачиваются при первом запуске (или берутся из `PATH`).
- Обновление программы по кнопке, с проверкой SHA-256.

## Как работает

1. yt-dlp скачивает нужный кусок записи с небольшим запасом.
2. Комментарии чата загружаются за тот же интервал.
3. Чат рисуется в отдельные кадры и накладывается ffmpeg; точный рез и кодирование (H.264 + AAC) идут за один проход.

Архитектурные решения описаны в [docs/adr](docs/adr), термины — в [GLOSSARY.md](GLOSSARY.md).

## Сборка из исходников

Нужны Rust (stable), Node.js 22 и [системные зависимости Tauri 2](https://v2.tauri.app/start/prerequisites/).

```sh
npm ci
npm run tauri dev     # запуск в режиме разработки
npm run tauri build   # релизная сборка
```

## Стек

[Tauri 2](https://tauri.app) + Rust, интерфейс на Svelte 5 + TypeScript.

## Лицензия

[MIT](LICENSE).

WClip — неофициальный инструмент и не связан с Twitch. Скачивайте и публикуйте только то, на что у вас есть права.
