// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  // Styles ship inside each component's JS. With separate virtual CSS modules the dev
  // server sometimes serves a CSS request before the component is compiled and falls
  // back to the raw .svelte file: styles go unscoped and the first rule is lost.
  compilerOptions: { css: "injected" },
  kit: {
    adapter: adapter({
      fallback: "index.html",
    }),
  },
};

export default config;
