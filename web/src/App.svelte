<script lang="ts">
  import { app, displayName } from './lib/app.svelte';
  import { notificationPermission, requestNotifications } from './lib/notify';
  import Sidebar from './components/Sidebar.svelte';
  import Chat from './components/Chat.svelte';
  import Invites from './components/Invites.svelte';

  let permission = $state(notificationPermission());

  async function enableNotifications() {
    permission = await requestNotifications();
  }

  $effect(() => {
    const n = app.unreadTotal;
    document.title = n > 0 ? `(${n}) NetworkDataShare` : 'NetworkDataShare';
  });

  // Coming back to the tab marks the open chat as read.
  $effect(() => {
    const onVisible = () => {
      if (!document.hidden && app.active) app.active.unread = 0;
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => document.removeEventListener('visibilitychange', onVisible);
  });
</script>

<div class="layout" class:has-active={app.activeId !== null}>
  <header>
    <div class="brand">
      <img src="/favicon.svg" alt="" width="22" height="22" />
      <strong>NetworkDataShare</strong>
    </div>
    <div class="header-right">
      {#if permission === 'default'}
        <button class="ghost small" onclick={enableNotifications} title="Get notified about invites while this tab is in the background">
          Enable notifications
        </button>
      {/if}
      <span class="status {app.status}" title="Connection to the server">
        <span class="dot"></span>
        {app.status === 'online' ? 'Connected' : app.status === 'connecting' ? 'Connecting...' : 'Reconnecting...'}
      </span>
    </div>
  </header>

  <aside>
    <Sidebar />
  </aside>

  <main>
    {#if app.sessions.length > 0}
      <nav class="tabs">
        {#each app.sessions as s (s.id)}
          <div class="tab" class:active={s.id === app.activeId} class:ended={s.closed}>
            <button class="tab-label" onclick={() => app.activate(s.id)}>
              <span class="peer-dot" class:online={s.peerOnline && !s.closed}></span>
              {displayName(s.peer)}
              {#if s.unread > 0}<span class="badge">{s.unread}</span>{/if}
            </button>
            <button class="tab-close" title={s.closed ? 'Close tab' : 'End session'} onclick={() => app.closeSession(s.id)}>
              x
            </button>
          </div>
        {/each}
      </nav>
    {/if}

    {#if app.active}
      {#key app.active.id}
        <Chat session={app.active} />
      {/key}
    {:else}
      <div class="empty">
        <h2>Share something</h2>
        <p class="muted">
          Open this page on another device in the same network, pick it from the list and send it an invite.
          Once it is accepted, you can send text, files and images, or paste screenshots straight from the clipboard.
        </p>
      </div>
    {/if}
  </main>
</div>

<Invites />

<div class="toasts">
  {#each app.toasts as t (t.id)}
    <div class="toast {t.kind}">{t.text}</div>
  {/each}
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: 300px 1fr;
    grid-template-rows: auto 1fr;
    grid-template-areas: 'header header' 'aside main';
    height: 100%;
  }

  header {
    grid-area: header;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.6rem 1rem;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .header-right {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    color: var(--muted);
  }

  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--warn);
  }

  .status.online .dot {
    background: var(--ok);
  }

  .status.offline .dot {
    background: var(--danger);
  }

  aside {
    grid-area: aside;
    border-right: 1px solid var(--border);
    background: var(--panel);
    overflow-y: auto;
    min-height: 0;
  }

  main {
    grid-area: main;
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }

  .tabs {
    display: flex;
    gap: 0.25rem;
    padding: 0.4rem 0.5rem 0;
    overflow-x: auto;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
    flex-shrink: 0;
  }

  .tab {
    display: flex;
    align-items: center;
    border: 1px solid var(--border);
    border-bottom: none;
    border-radius: 8px 8px 0 0;
    background: var(--panel-2);
    white-space: nowrap;
  }

  .tab.active {
    background: var(--bg);
    border-color: var(--accent);
  }

  .tab.ended .tab-label {
    color: var(--muted);
    text-decoration: line-through;
  }

  .tab button {
    border: none;
    background: transparent;
    border-radius: 0;
  }

  .tab-label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .tab-close {
    color: var(--muted);
    padding-left: 0.2rem;
  }

  .peer-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted);
  }

  .peer-dot.online {
    background: var(--ok);
  }

  .badge {
    background: var(--accent);
    color: var(--accent-text);
    border-radius: 999px;
    font-size: 0.72rem;
    padding: 0 0.45rem;
  }

  .empty {
    margin: auto;
    max-width: 30rem;
    padding: 2rem;
    text-align: center;
  }

  .toasts {
    position: fixed;
    bottom: 1rem;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    z-index: 30;
    pointer-events: none;
  }

  .toast {
    background: var(--panel);
    border: 1px solid var(--border);
    border-left: 4px solid var(--accent);
    box-shadow: var(--shadow);
    border-radius: 8px;
    padding: 0.6rem 1rem;
  }

  .toast.error {
    border-left-color: var(--danger);
  }

  /* Phones: show either the client list or the chat. */
  @media (max-width: 720px) {
    .layout {
      grid-template-columns: 1fr;
      grid-template-areas: 'header' 'aside';
    }

    .layout.has-active {
      grid-template-areas: 'header' 'main';
    }

    main {
      display: none;
    }

    .layout.has-active main {
      display: flex;
    }

    .layout.has-active aside {
      display: none;
    }

    .header-right button {
      display: none;
    }
  }
</style>
