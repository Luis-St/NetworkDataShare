// Invite alerts: a short sound plus a system notification while the tab is in the background.

let audio: AudioContext | null = null;

/** Browsers only allow audio after a user gesture, so the context is created on the first one. */
export function unlockAudio() {
  if (audio) return;
  try {
    audio = new AudioContext();
  } catch {
    audio = null;
  }
}

export function beep() {
  if (!audio) return;
  if (audio.state === 'suspended') void audio.resume();
  const now = audio.currentTime;
  for (const [offset, freq] of [
    [0, 880],
    [0.18, 1175],
  ] as const) {
    const osc = audio.createOscillator();
    const gain = audio.createGain();
    osc.type = 'sine';
    osc.frequency.value = freq;
    gain.gain.setValueAtTime(0.0001, now + offset);
    gain.gain.exponentialRampToValueAtTime(0.25, now + offset + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + offset + 0.16);
    osc.connect(gain).connect(audio.destination);
    osc.start(now + offset);
    osc.stop(now + offset + 0.18);
  }
}

export const notificationsSupported = typeof Notification !== 'undefined' && window.isSecureContext;

export function notificationPermission(): NotificationPermission | 'unsupported' {
  return notificationsSupported ? Notification.permission : 'unsupported';
}

export async function requestNotifications(): Promise<NotificationPermission | 'unsupported'> {
  if (!notificationsSupported) return 'unsupported';
  try {
    return await Notification.requestPermission();
  } catch {
    return Notification.permission;
  }
}

export function notify(title: string, body: string, tag: string) {
  if (!notificationsSupported || Notification.permission !== 'granted' || !document.hidden) return;
  try {
    const n = new Notification(title, { body, tag, icon: '/favicon.svg' });
    n.onclick = () => {
      window.focus();
      n.close();
    };
  } catch {
    /* some mobile browsers only allow notifications from a service worker */
  }
}
