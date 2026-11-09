<script lang="ts">
  import { app, type ChatItem } from '../lib/app.svelte';
  import { canCopyImages, copyImage, copyText, formatSize, formatTime, linkify } from '../lib/util';

  let { item, onload }: { item: ChatItem; onload?: () => void } = $props();

  let copied = $state(false);

  async function copy(fn: () => Promise<boolean>) {
    if (await fn()) {
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } else {
      app.toast('Copying failed. Your browser may block clipboard access here.', 'error');
    }
  }

  const transfer = $derived(item.kind === 'file' ? item.transfer : undefined);
  const percent = $derived(transfer && transfer.total > 0 ? Math.round((transfer.bytes / transfer.total) * 100) : 0);
  const isImage = $derived(item.kind === 'file' && item.mime.startsWith('image/'));
</script>

{#if item.kind === 'system'}
  <div class="system small muted">{item.text}</div>
{:else}
  <div class="row" class:mine={item.mine}>
    <div class="bubble" class:failed={item.failed}>
      {#if item.kind === 'text'}
        <div class="text">
          {#each linkify(item.text) as seg, i (i)}
            {#if seg.href}<a href={seg.href} target="_blank" rel="noopener noreferrer">{seg.text}</a>{:else}{seg.text}{/if}
          {/each}
        </div>
      {:else}
        {#if isImage && item.previewUrl}
          <a href={item.previewUrl} target="_blank" rel="noopener" class="preview">
            <img src={item.previewUrl} alt={item.name} onload={onload} />
          </a>
        {/if}
        <div class="file">
          <div class="file-icon">{isImage ? 'IMG' : (item.name.split('.').pop() ?? '').slice(0, 4).toUpperCase() || 'FILE'}</div>
          <div class="file-info">
            <div class="file-name" title={item.name}>{item.name}</div>
            <div class="small sub">
              {formatSize(item.size)}
              {#if transfer?.state === 'running'}
                - {percent}%
              {:else if transfer?.state === 'failed'}
                - <span class="err">{transfer.error ?? 'transfer failed'}</span>
              {:else if item.mine}
                - {item.delivered ? `downloaded ${item.delivered > 1 ? `${item.delivered}x` : ''}` : 'ready'}
              {:else if transfer?.state === 'done'}
                - received
              {/if}
            </div>
          </div>
        </div>
        {#if transfer?.state === 'running'}
          <div class="progress"><div style:width="{percent}%"></div></div>
        {/if}
      {/if}

      <div class="meta">
        {#if item.failed}<span class="err small">not delivered</span>{/if}
        <span class="small sub">{formatTime(item.ts)}</span>
        <span class="actions">
          {#if item.kind === 'text'}
            <button class="ghost small" onclick={() => copy(() => copyText(item.text))}>{copied ? 'Copied' : 'Copy'}</button>
          {:else}
            {#if isImage && item.previewUrl && canCopyImages}
              <button class="ghost small" onclick={() => copy(() => copyImage(item.previewUrl!))}>{copied ? 'Copied' : 'Copy'}</button>
            {/if}
            {#if !item.mine}
              <button class="ghost small" onclick={() => app.download(item)} disabled={transfer?.state === 'running'}>Download</button>
            {/if}
          {/if}
        </span>
      </div>
    </div>
  </div>
{/if}

<style>
  .system {
    text-align: center;
    margin: 0.25rem 0;
  }

  .row {
    display: flex;
  }

  .row.mine {
    justify-content: flex-end;
  }

  .bubble {
    max-width: min(560px, 85%);
    background: var(--theirs);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.5rem 0.7rem 0.3rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-width: 0;
  }

  .mine .bubble {
    background: var(--mine);
    border-color: var(--mine);
    color: var(--mine-text);
  }

  .bubble.failed {
    opacity: 0.7;
  }

  .text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .preview img {
    display: block;
    max-width: 100%;
    max-height: 360px;
    border-radius: 8px;
  }

  .file {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    min-width: 0;
  }

  .file-icon {
    flex-shrink: 0;
    width: 2.6rem;
    height: 2.6rem;
    border-radius: 8px;
    display: grid;
    place-items: center;
    font-size: 0.65rem;
    font-weight: 700;
    background: var(--accent-soft);
    color: var(--accent);
  }

  .mine .file-icon {
    background: rgb(255 255 255 / 0.2);
    color: inherit;
  }

  .file-info {
    min-width: 0;
  }

  .file-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sub {
    opacity: 0.75;
  }

  .progress {
    height: 5px;
    border-radius: 3px;
    background: rgb(127 127 127 / 0.25);
    overflow: hidden;
  }

  .progress div {
    height: 100%;
    background: var(--ok);
    transition: width 0.25s linear;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .actions {
    margin-left: auto;
    display: flex;
    gap: 0.1rem;
  }

  .actions button {
    padding: 0.1rem 0.4rem;
    color: inherit;
  }

  .err {
    color: var(--danger);
  }

  .mine .err {
    color: #fecaca;
  }
</style>
