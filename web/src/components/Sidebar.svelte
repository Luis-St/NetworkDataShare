<script lang="ts">
  import { app, displayName } from '../lib/app.svelte';

  let draft = $state(app.name);

  function saveName() {
    if (draft.trim() !== app.name) app.setName(draft);
    draft = app.name;
  }
</script>

<section class="me">
  <label class="small muted" for="my-name">Your name (optional)</label>
  <input
    id="my-name"
    placeholder={app.device}
    maxlength="64"
    bind:value={draft}
    onblur={saveName}
    onkeydown={(e) => e.key === 'Enter' && (e.currentTarget as HTMLInputElement).blur()}
  />
  <div class="small muted">{app.device} - {app.me?.ip ?? '...'}</div>
</section>

<section>
  <h3>Devices on the network</h3>
  {#if app.others.length === 0}
    <p class="muted small hint">No other devices yet. Open this page on another device in the same network.</p>
  {/if}
  <ul>
    {#each app.others as c (c.id)}
      {@const session = app.sessionWith(c.id)}
      {@const pending = app.outgoing[c.id]}
      <li>
        <div class="info">
          <div class="name">{displayName(c)}</div>
          <div class="small muted">{c.name ? `${c.device} - ` : ''}{c.ip}</div>
        </div>
        {#if session}
          <button onclick={() => app.activate(session.id)}>Open</button>
        {:else if pending}
          <button class="danger" onclick={() => app.cancelInvite(c.id)} title="Waiting for an answer">Cancel</button>
        {:else}
          <button class="primary" onclick={() => app.invite(c.id)} disabled={app.status !== 'online'}>Share</button>
        {/if}
      </li>
    {/each}
  </ul>
</section>

{#if app.sessions.length > 0}
  <section class="sessions">
    <h3>Sessions</h3>
    <ul>
      {#each app.sessions as s (s.id)}
        <li>
          <button class="ghost session" class:active={s.id === app.activeId} onclick={() => app.activate(s.id)}>
            <span class="name">{displayName(s.peer)}</span>
            <span class="small muted">{s.closed ? 'ended' : s.peerOnline ? 'online' : 'offline'}</span>
            {#if s.unread > 0}<span class="badge">{s.unread}</span>{/if}
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  section {
    padding: 0.9rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  .me {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  h3 {
    margin: 0 0 0.5rem;
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.45rem 0.5rem;
    border-radius: 8px;
    background: var(--panel-2);
  }

  .info {
    flex: 1;
    min-width: 0;
  }

  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    margin: 0;
  }

  .sessions li {
    padding: 0;
  }

  .session {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    text-align: left;
  }

  .session .name {
    flex: 1;
  }

  .session.active {
    outline: 2px solid var(--accent);
  }

  .badge {
    background: var(--accent);
    color: var(--accent-text);
    border-radius: 999px;
    font-size: 0.72rem;
    padding: 0 0.45rem;
  }
</style>
