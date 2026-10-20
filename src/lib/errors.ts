// Turns errors from the Rust side (`Failure`, see src-tauri/src/failure.rs) into words.
import type { Failure, FailureKind } from "./api.ts";

const TEXT: Record<Exclude<FailureKind, "other">, string> = {
  "vod-missing": "Запись не найдена: её удалили, скрыли или она ещё обрабатывается.",
  "sub-only":
    "Запись только для подписчиков канала. Вставьте токен Twitch в настройках; если он уже там, у этого аккаунта нет доступа к записи.",
  "no-model": "Для субтитров нужна модель распознавания речи: скачайте её в настройках.",
  "bad-token": "Twitch не принял токен из настроек: он устарел или скопирован с ошибкой. Вставьте новый.",
  network: "Нет связи с Twitch. Проверьте интернет и попробуйте ещё раз.",
  "disk-full": "На диске не хватает места. Освободите место или выберите другую папку.",
  chat: "Не удалось загрузить чат.",
};

function isFailure(e: unknown): e is Failure {
  return typeof e === "object" && e !== null && "kind" in e && "detail" in e;
}

/** `text` to show; `detail` is the raw message, for a tooltip, when `text` hides it. */
export function explain(e: unknown): { text: string; detail: string | null } {
  if (!isFailure(e)) return { text: String(e), detail: null };
  if (e.kind === "other") return { text: e.detail, detail: null };
  return { text: TEXT[e.kind], detail: e.detail };
}
