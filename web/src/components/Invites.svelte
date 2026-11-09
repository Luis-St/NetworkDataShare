<script lang="ts">
  import { app, displayName } from '../lib/app.svelte';

  let now = $state(Date.now());

  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(timer);
  });
</script>

{#if app.incoming.length > 0}
  <div class="invites">
    {#each app.incoming as inv (inv.inviteId)}
      {@const left = Math.min(app.inviteTimeoutSecs, Math.max(0, Math.round((inv.expiresAt - now) / 1000)))}
      <div class="invite" role="alertdialog" aria-label="Share invite">
        <div>
          <strong>{displayName(inv.from)}</strong> wants to share with you.
          <div class="small muted">{inv.from.name ? `${inv.from.device} - ` : ''}{inv.from.ip}</div>
        </div>
        <div class="actions">
          <span class="small muted">{left}s</span>
          <button onclick={() => app.respond(inv.inviteId, false)}>Deny</button>
          <button class="primary" onclick={() => app.respond(inv.inviteId, true)}>Accept</button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .invites {
    position: fixed;
    top: 4rem;
    right: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    z-index: 40;
    width: min(360px, calc(100vw - 2rem));
  }

  .invite {
    background: var(--panel);
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 0.8rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    animation: pop 0.2s ease-out;
  }

  .actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .actions span {
    margin-right: auto;
  }

  @keyframes pop {
    from {
      transform: translateY(-8px);
      opacity: 0;
    }
  }
</style>
