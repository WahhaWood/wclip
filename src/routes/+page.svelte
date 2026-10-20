<script lang="ts">
  import { onMount } from "svelte";
  import { api, type ToolInfo } from "$lib/api";
  import Home from "$lib/components/Home.svelte";
  import Queue from "$lib/components/Queue.svelte";
  import SettingsScreen from "$lib/components/SettingsScreen.svelte";
  import ToolsSetup from "$lib/components/ToolsSetup.svelte";
  import { app, initApp, saveSettings, type Screen } from "$lib/state.svelte";

  let tools = $state<ToolInfo[] | null>(null);
  let toolsReady = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    Promise.all([initApp(), api.toolsStatus()])
      .then(([, status]) => {
        tools = status;
        toolsReady = status.every((t) => t.path);
      })
      .catch((e) => (error = String(e)));
  });

  // Any settings change is saved, a moment later.
  let loaded = false;
  $effect(() => {
    JSON.stringify(app.settings);
    if (loaded) saveSettings();
    loaded = app.settings !== null;
  });

  const tabs = $derived<[Screen, string][]>([
    ["home", "Нарезка"],
    ...(app.queue.length ? [["queue", app.running ? "Очередь ●" : "Очередь"] as [Screen, string]] : []),
    ["settings", "Настройки"],
  ]);
</script>

{#if error}
  <main class="center"><p class="error">{error}</p></main>
{:else if !tools || !app.settings}
  <main class="center"><p class="dim">Загрузка…</p></main>
{:else if !toolsReady}
  <ToolsSetup {tools} onready={() => (toolsReady = true)} />
{:else}
  <div class="app">
    <header class="glass">
      <img src="/logo.svg" width="28" height="28" alt="" />
      <span class="brand">WClip</span>
      <nav>
        {#each tabs as [screen, label] (screen)}
          <button class="btn small" class:primary={app.screen === screen} onclick={() => (app.screen = screen)}>
            {label}
          </button>
        {/each}
      </nav>
    </header>
    <main>
      {#if app.screen === "home"}
        <Home />
      {:else if app.screen === "queue"}
        <Queue />
      {:else}
        <SettingsScreen />
      {/if}
    </main>
  </div>
{/if}

<style>
  .center {
    min-height: 100vh;
    display: grid;
    place-items: center;
  }

  .app {
    max-width: 1100px;
    margin: 0 auto;
    padding: 16px 24px 32px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    margin-bottom: 16px;
    position: sticky;
    top: 16px;
    z-index: 1;
  }

  .brand {
    font-weight: 700;
    font-size: 18px;
  }

  nav {
    margin-left: auto;
    display: flex;
    gap: 6px;
  }
</style>
