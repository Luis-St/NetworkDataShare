<script lang="ts">
  import { tick } from 'svelte';
  import { app, displayName, type ChatSession } from '../lib/app.svelte';
  import Message from './Message.svelte';

  let { session }: { session: ChatSession } = $props();

  let text = $state('');
  let list: HTMLDivElement;
  let input: HTMLTextAreaElement;
  let fileInput: HTMLInputElement;
  let dragDepth = $state(0);
  let stickToBottom = true;

  const disabled = $derived(session.closed !== null);

  function send() {
    if (!text.trim()) return;
    app.sendText(session.id, text);
    text = '';
    stickToBottom = true;
    input.style.height = '';
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  function autoGrow() {
    input.style.height = '';
    input.style.height = `${Math.min(input.scrollHeight, 200)}px`;
  }

  function sendFiles(files: FileList | File[] | null | undefined) {
    if (!files || files.length === 0 || disabled) return;
    stickToBottom = true;
    app.sendFiles(session.id, Array.from(files));
  }

  function onScroll() {
    stickToBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 80;
  }

  function scrollDown() {
    if (stickToBottom && list) list.scrollTop = list.scrollHeight;
  }

  $effect(() => {
    void session.items.length;
    tick().then(scrollDown);
  });

  // Focus the composer on desktops; on phones it would pop up the keyboard.
  $effect(() => {
    if (matchMedia('(pointer: fine)').matches) input?.focus();
  });

  // Pasting files or screenshots anywhere in the chat sends them; plain text pastes stay normal.
  $effect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && target !== input && target.closest('input, textarea, [contenteditable]')) return;
      const files = Array.from(e.clipboardData?.files ?? []);
      if (files.length > 0) {
        e.preventDefault();
        sendFiles(files);
      }
    };
    window.addEventListener('paste', onPaste);
    return () => window.removeEventListener('paste', onPaste);
  });

  function hasFiles(e: DragEvent) {
    return e.dataTransfer?.types.includes('Files') ?? false;
  }
</script>

<div
  class="chat"
  role="region"
  aria-label="Chat"
  ondragenter={(e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth++;
  }}
  ondragover={(e) => {
    if (hasFiles(e)) e.preventDefault();
  }}
  ondragleave={() => (dragDepth = Math.max(0, dragDepth - 1))}
  ondrop={(e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth = 0;
    sendFiles(e.dataTransfer?.files);
  }}
>
  <div class="chat-header">
    <button class="ghost back" onclick={() => app.activate(null)} aria-label="Back to the device list">&lt;</button>
    <div class="peer">
      <strong>{displayName(session.peer)}</strong>
      <span class="small muted">
        {session.peer.name ? `${session.peer.device} - ` : ''}{session.peer.ip} -
        {#if session.closed}ended{:else if session.peerOnline}online{:else}reconnecting...{/if}
      </span>
    </div>
    <button class="danger" onclick={() => app.closeSession(session.id)}>
      {session.closed ? 'Close tab' : 'End session'}
    </button>
  </div>

  <div class="messages" bind:this={list} onscroll={onScroll}>
    {#each session.items as item (item.id)}
      <Message {item} onload={scrollDown} />
    {/each}
  </div>

  <div class="composer">
    <input type="file" multiple hidden bind:this={fileInput} onchange={() => {
      sendFiles(fileInput.files);
      fileInput.value = '';
    }} />
    <button class="attach" title="Send files" onclick={() => fileInput.click()} {disabled}>+</button>
    <textarea
      rows="1"
      placeholder={disabled ? 'This session has ended' : 'Type a message, paste an image or drop files...'}
      bind:this={input}
      bind:value={text}
      oninput={autoGrow}
      onkeydown={onKeydown}
      {disabled}
    ></textarea>
    <button class="primary" onclick={send} disabled={disabled || !text.trim()}>Send</button>
  </div>

  {#if dragDepth > 0 && !disabled}
    <div class="drop">Drop files to send them to {displayName(session.peer)}</div>
  {/if}
</div>

<style>
  .chat {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .chat-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }

  .peer {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .back {
    display: none;
    font-weight: bold;
  }

  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .composer {
    display: flex;
    align-items: flex-end;
    gap: 0.5rem;
    padding: 0.6rem 0.75rem;
    border-top: 1px solid var(--border);
    background: var(--panel);
  }

  textarea {
    flex: 1;
    resize: none;
    max-height: 200px;
  }

  .attach {
    font-size: 1.2rem;
    line-height: 1;
    padding: 0.4rem 0.7rem;
  }

  .drop {
    position: absolute;
    inset: 0.5rem;
    display: grid;
    place-items: center;
    border: 3px dashed var(--accent);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--accent-soft) 85%, transparent);
    font-size: 1.2rem;
    font-weight: 600;
    pointer-events: none;
  }

  @media (max-width: 720px) {
    .back {
      display: block;
    }
  }
</style>
