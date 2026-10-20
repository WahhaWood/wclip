<script lang="ts">
  import { api } from "$lib/api";
  import { explain } from "$lib/errors";

  /** A frame of the video at `time`; fetched a moment after `time` stops changing. */
  let { playlist, time }: { playlist: string; time: number } = $props();

  let url = $state<string | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);

  $effect(() => {
    const [p, t] = [playlist, time];
    let stale = false;
    loading = true;
    const timer = setTimeout(() => {
      api
        .previewFrame(p, t)
        .then(
          (jpeg) => {
            if (stale) return;
            if (url) URL.revokeObjectURL(url);
            url = URL.createObjectURL(new Blob([jpeg], { type: "image/jpeg" }));
            error = null;
          },
          (e) => {
            if (!stale) error = explain(e).text;
          },
        )
        .finally(() => {
          if (!stale) loading = false;
        });
    }, 250);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  });

  $effect(() => () => {
    if (url) URL.revokeObjectURL(url);
  });
</script>

<div class="frame" class:loading>
  {#if url}
    <img src={url} alt="" />
  {/if}
  {#if error}
    <p class="error">{error}</p>
  {/if}
</div>

<style>
  .frame {
    position: relative;
    aspect-ratio: 16 / 9;
    border-radius: 12px;
    overflow: hidden;
    background: var(--surface-highest);
  }

  img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
    transition: opacity 0.15s;
  }

  .loading img {
    opacity: 0.6;
  }

  p {
    position: absolute;
    inset: auto 10px 8px;
    margin: 0;
    font-size: 13px;
  }
</style>
